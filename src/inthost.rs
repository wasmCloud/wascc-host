// Implementations of support functions for the `Host` struct

use super::Host;
use crate::Result;
use data_encoding::HEXUPPER;
use ring::digest::{Context, Digest, SHA256};

use crate::bus;
use crate::bus::MessageBus;
use crate::BindingsList;
use crate::{authz, errors, Actor, Authorizer, NativeCapability, RouteKey};
use errors::ErrorKind;
use provider_archive::ProviderArchive;
use std::str::FromStr;
use std::{
    collections::HashMap,
    io::Read,
    sync::{Arc, RwLock},
};
use uuid::Uuid;
use wapc::WapcHost;
use wascap::{jwt::Claims, prelude::KeyPair};
use wascc_codec::{
    capabilities::{CapabilityDescriptor, OP_GET_CAPABILITY_DESCRIPTOR},
    core::{CapabilityConfiguration, OP_PERFORM_LIVE_UPDATE, OP_REMOVE_ACTOR},
    deserialize, serialize, SYSTEM_ACTOR,
};

pub(crate) const CORELABEL_ARCH: &str = "hostcore.arch";
pub(crate) const CORELABEL_OS: &str = "hostcore.os";
pub(crate) const CORELABEL_OSFAMILY: &str = "hostcore.osfamily";

pub(crate) const OCI_VAR_USER: &str = "OCI_REGISTRY_USER";
pub(crate) const OCI_VAR_PASSWORD: &str = "OCI_REGISTRY_PASSWORD";

#[allow(dead_code)]
pub(crate) const RESTRICTED_LABELS: [&str; 3] = [CORELABEL_OSFAMILY, CORELABEL_ARCH, CORELABEL_OS];

// Unsubscribes all of the private actor-provider comms subjects
pub(crate) fn unsub_all_bindings(
    bindings: Arc<RwLock<BindingsList>>,
    bus: Arc<MessageBus>,
    capid: &str,
) {
    bindings
        .read()
        .unwrap()
        .keys()
        .filter(|(_a, c, _b)| c == capid)
        .for_each(|(a, c, b)| {
            let _ = bus.unsubscribe(&bus.provider_subject_bound_actor(c, b, a));
        });
}

impl Host {
    pub(crate) fn record_binding(
        &self,
        actor: &str,
        capid: &str,
        binding: &str,
        config: &CapabilityConfiguration,
    ) -> Result<()> {
        let mut lock = self.bindings.write().unwrap();
        lock.insert(
            (actor.to_string(), capid.to_string(), binding.to_string()),
            config.clone(),
        );
        trace!(
            "Actor {} successfully bound to {},{}",
            actor,
            binding,
            capid
        );
        Ok(())
    }

    pub(crate) fn ensure_extras(&self) -> Result<()> {
        self.add_native_capability(NativeCapability::from_instance(
            crate::extras::ExtrasCapabilityProvider::default(),
            None,
        )?)?;
        Ok(())
    }
}

/// In the case of a portable capability provider, obtain its capability descriptor
pub(crate) fn get_descriptor(host: &mut WapcHost) -> Result<CapabilityDescriptor> {
    let msg = wascc_codec::core::HealthRequest { placeholder: false }; // TODO: eventually support sending an empty slice for this
    let res = host.call(OP_GET_CAPABILITY_DESCRIPTOR, &serialize(&msg)?)?;
    deserialize(&res).map_err(|e| e.into())
}

pub(crate) fn remove_cap(
    caps: Arc<RwLock<HashMap<RouteKey, CapabilityDescriptor>>>,
    capid: &str,
    binding: &str,
) {
    caps.write().unwrap().remove(&RouteKey::new(binding, capid));
}

/// Puts a "live update" message into the dispatch queue, which will be handled
/// as soon as it is pulled off the channel for the target actor
pub(crate) fn replace_actor(
    hostkey: &KeyPair,
    bus: Arc<MessageBus>,
    new_actor: Actor,
) -> Result<()> {
    let public_key = new_actor.token.claims.subject;
    let tgt_subject = bus.actor_subject(&public_key);
    let inv = gen_liveupdate_invocation(hostkey, &public_key, new_actor.bytes);

    match bus.invoke(&tgt_subject, inv) {
        Ok(_) => {
            info!("Actor {} replaced", public_key);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn live_update(guest: &mut WapcHost, inv: &Invocation) -> InvocationResponse {
    match guest.replace_module(&inv.msg) {
        Ok(_) => InvocationResponse::success(inv, vec![]),
        Err(e) => {
            error!("Failed to perform hot swap, ignoring message: {}", e);
            InvocationResponse::error(inv, "Failed to perform hot swap")
        }
    }
}

fn gen_liveupdate_invocation(hostkey: &KeyPair, target: &str, bytes: Vec<u8>) -> Invocation {
    Invocation::new(
        hostkey,
        WasccEntity::Actor(SYSTEM_ACTOR.to_string()),
        WasccEntity::Actor(target.to_string()),
        OP_PERFORM_LIVE_UPDATE,
        bytes,
    )
}

/// Removes all bindings for a given actor by sending the "remove actor" message
/// to each of the capabilities
pub(crate) fn deconfigure_actor(
    hostkey: KeyPair,
    bus: Arc<MessageBus>,
    bindings: Arc<RwLock<BindingsList>>,
    key: &str,
) {
    #[cfg(feature = "lattice")]
    {
        // Don't remove the bindings for this actor unless it's the last instance in the lattice
        if let Ok(i) = bus.instance_count(key) {
            if i > 0 {
                // This is 0 because the actor being removed has already been taken out of the claims map, so bus queries will not see the local instance
                info!("Actor instance terminated at scale > 1, bypassing binding removal.");
                return;
            }
        }
    }
    let cfg = CapabilityConfiguration {
        module: key.to_string(),
        values: HashMap::new(),
    };
    let buf = serialize(&cfg).unwrap();
    let nbindings: Vec<_> = {
        let lock = bindings.read().unwrap();
        lock.keys()
            .filter(|(a, _cap, _bind)| a == key)
            .cloned()
            .collect()
    };

    // (actor, capid, binding)
    for (actor, capid, binding) in nbindings {
        info!("Unbinding actor {} from {},{}", actor, binding, capid);
        let _inv_r = bus.invoke(
            &bus.provider_subject(&capid, &binding), // The OP_REMOVE_ACTOR invocation should go to _all_ instances of the provider being unbound
            gen_remove_actor(&hostkey, buf.clone(), &binding, &capid),
        );
        remove_binding(bindings.clone(), key, &binding, &capid);
    }
}

/// Removes all bindings from a capability without notifying anyone
pub(crate) fn unbind_all_from_cap(bindings: Arc<RwLock<BindingsList>>, capid: &str, binding: &str) {
    let mut lock = bindings.write().unwrap();
    // (actor, capid, binding name)
    lock.retain(|k, _| !(k.1 == capid) && (k.2 == binding));
}

pub(crate) fn remove_binding(
    bindings: Arc<RwLock<BindingsList>>,
    actor: &str,
    binding: &str,
    capid: &str,
) {
    // binding: (actor,  capid, binding)
    let mut lock = bindings.write().unwrap();
    lock.remove(&(actor.to_string(), capid.to_string(), binding.to_string()));
}

pub(crate) fn gen_remove_actor(
    hostkey: &KeyPair,
    msg: Vec<u8>,
    binding: &str,
    capid: &str,
) -> Invocation {
    Invocation::new(
        hostkey,
        WasccEntity::Actor(SYSTEM_ACTOR.to_string()),
        WasccEntity::Capability {
            capid: capid.to_string(),
            binding: binding.to_string(),
        },
        OP_REMOVE_ACTOR,
        msg,
    )
}

/// An immutable representation of an invocation within waSCC
#[derive(Debug, Clone)]
#[cfg_attr(feature = "lattice", derive(serde::Serialize, serde::Deserialize))]
pub struct Invocation {
    pub origin: WasccEntity,
    pub target: WasccEntity,
    pub operation: String,
    pub msg: Vec<u8>,
    pub id: String,
    pub encoded_claims: String,
    pub host_id: String,
}

/// Represents an invocation target - either an actor or a bound capability provider
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "lattice", derive(serde::Serialize, serde::Deserialize))]
pub enum WasccEntity {
    Actor(String),
    Capability { capid: String, binding: String },
}

impl WasccEntity {
    pub fn url(&self) -> String {
        match self {
            WasccEntity::Actor(pk) => format!("{}://{}", bus::URL_SCHEME, pk),
            WasccEntity::Capability { capid, binding } => format!(
                "{}://{}/{}",
                bus::URL_SCHEME,
                capid.replace(":", "/").replace(" ", "_").to_lowercase(),
                binding.replace(" ", "_").to_lowercase(),
            ),
        }
    }
}

impl Invocation {
    pub fn new(
        hostkey: &KeyPair,
        origin: WasccEntity,
        target: WasccEntity,
        op: &str,
        msg: Vec<u8>,
    ) -> Invocation {
        let subject = format!("{}", Uuid::new_v4());
        let issuer = hostkey.public_key();
        let target_url = format!("{}/{}", target.url(), op);
        let claims = Claims::<wascap::prelude::Invocation>::new(
            issuer.to_string(),
            subject.to_string(),
            &target_url,
            &origin.url(),
            &invocation_hash(&target_url, &origin.url(), &msg),
        );
        Invocation {
            origin,
            target,
            operation: op.to_string(),
            msg,
            id: subject,
            encoded_claims: claims.encode(&hostkey).unwrap(),
            host_id: issuer.to_string(),
        }
    }

    pub fn origin_url(&self) -> String {
        self.origin.url()
    }

    pub fn target_url(&self) -> String {
        format!("{}/{}", self.target.url(), self.operation)
    }

    pub fn hash(&self) -> String {
        invocation_hash(&self.target_url(), &self.origin_url(), &self.msg)
    }

    pub fn validate_antiforgery(&self) -> Result<()> {
        let vr = wascap::jwt::validate_token::<wascap::prelude::Invocation>(&self.encoded_claims)?;
        let claims = Claims::<wascap::prelude::Invocation>::decode(&self.encoded_claims)?;
        if vr.expired {
            return Err(errors::new(ErrorKind::Authorization(
                "Invocation claims token expired".into(),
            )));
        }
        if !vr.signature_valid {
            return Err(errors::new(ErrorKind::Authorization(
                "Invocation claims signature invalid".into(),
            )));
        }
        if vr.cannot_use_yet {
            return Err(errors::new(ErrorKind::Authorization(
                "Attempt to use invocation before claims token allows".into(),
            )));
        }
        let inv_claims = claims.metadata.unwrap();
        if inv_claims.invocation_hash != self.hash() {
            return Err(errors::new(ErrorKind::Authorization(
                "Invocation hash does not match signed claims hash".into(),
            )));
        }
        if claims.subject != self.id {
            return Err(errors::new(ErrorKind::Authorization(
                "Subject of invocation claims token does not match invocation ID".into(),
            )));
        }
        if claims.issuer != self.host_id {
            return Err(errors::new(ErrorKind::Authorization(
                "Invocation claims issuer does not match invocation host".into(),
            )));
        }
        if inv_claims.target_url != self.target_url() {
            return Err(errors::new(ErrorKind::Authorization(
                "Invocation claims and invocation target URL do not match".into(),
            )));
        }
        if inv_claims.origin_url != self.origin_url() {
            return Err(errors::new(ErrorKind::Authorization(
                "Invocation claims and invocation origin URL do not match".into(),
            )));
        }

        Ok(())
    }
}

/// The response to an invocation
#[derive(Debug, Clone)]
#[cfg_attr(feature = "lattice", derive(serde::Serialize, serde::Deserialize))]
pub struct InvocationResponse {
    pub msg: Vec<u8>,
    pub error: Option<String>,
    pub invocation_id: String,
}

impl InvocationResponse {
    pub fn success(inv: &Invocation, msg: Vec<u8>) -> InvocationResponse {
        InvocationResponse {
            msg,
            error: None,
            invocation_id: inv.id.to_string(),
        }
    }

    pub fn error(inv: &Invocation, err: &str) -> InvocationResponse {
        InvocationResponse {
            msg: Vec::new(),
            error: Some(err.to_string()),
            invocation_id: inv.id.to_string(),
        }
    }
}

pub(crate) fn wapc_host_callback(
    hostkey: KeyPair,
    claims: Claims<wascap::jwt::Actor>,
    bus: Arc<MessageBus>,
    binding: &str,
    namespace: &str,
    operation: &str,
    payload: &[u8],
    authorizer: Arc<RwLock<Box<dyn Authorizer>>>,
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    trace!(
        "Guest {} invoking {}:{}",
        claims.subject,
        namespace,
        operation
    );

    let capability_id = namespace;
    let inv = invocation_from_callback(
        &hostkey,
        &claims.subject,
        binding,
        namespace,
        operation,
        payload,
    );

    if !authz::can_invoke(&claims, capability_id, operation) {
        return Err(Box::new(errors::new(errors::ErrorKind::Authorization(
            format!(
                "{} {} attempted to call {} on {},{} - PERMISSION DENIED.",
                if claims.metadata.unwrap().provider {
                    "Provider"
                } else {
                    "Actor"
                },
                claims.subject,
                operation,
                capability_id,
                binding
            ),
        ))));
    } else {
        if !authorizer
            .read()
            .unwrap()
            .can_invoke(&claims, &inv.target, operation)
        {
            return Err(Box::new(errors::new(errors::ErrorKind::Authorization(
                format!(
                    "{} {} attempted to call {:?} - Authorizer denied access",
                    if claims.metadata.unwrap().provider {
                        "Provider"
                    } else {
                        "Actor"
                    },
                    claims.subject,
                    &inv.target
                ),
            ))));
        }
    }
    // Make a request on either `wasmbus.Mxxxxx` for an actor or `wasmbus.{capid}.{binding}.{calling-actor}` for
    // a bound capability provider
    let invoke_subject = match &inv.target {
        WasccEntity::Actor(subject) => bus.actor_subject(subject),
        WasccEntity::Capability { capid, binding } => {
            bus.provider_subject_bound_actor(capid, binding, &claims.subject)
        }
    };
    match bus.invoke(&invoke_subject, inv) {
        Ok(inv_r) => match inv_r.error {
            Some(e) => Err(format!("Invocation failure: {}", e).into()),
            None => Ok(inv_r.msg),
        },
        Err(e) => Err(Box::new(errors::new(errors::ErrorKind::HostCallFailure(
            e.into(),
        )))),
    }
}

pub(crate) fn fetch_oci_bytes(img: &str) -> Result<Vec<u8>> {
    let cfg = oci_distribution::client::ClientConfig::default();
    let mut c = oci_distribution::Client::new(cfg);

    let img = oci_distribution::Reference::from_str(img).map_err(|e| {
        crate::errors::new(crate::errors::ErrorKind::MiscHost(format!(
            "Failed to parse OCI distribution reference: {}",
            e
        )))
    })?;
    let auth = if let Ok(u) = std::env::var(OCI_VAR_USER) {
        if let Ok(p) = std::env::var(OCI_VAR_PASSWORD) {
            oci_distribution::secrets::RegistryAuth::Basic(u, p)
        } else {
            oci_distribution::secrets::RegistryAuth::Anonymous
        }
    } else {
        oci_distribution::secrets::RegistryAuth::Anonymous
    };
    let imgdata: Result<oci_distribution::client::ImageData> =
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            c.pull_image(&img, &auth)
                .await
                .map_err(|e| format!("{}", e).into())
        });

    match imgdata {
        Ok(imgdata) => Ok(imgdata.content),
        Err(e) => {
            error!("Failed to fetch OCI bytes: {}", e);
            Err(crate::errors::new(crate::errors::ErrorKind::MiscHost(
                "Failed to fetch OCI bytes".to_string(),
            )))
        }
    }
}

pub(crate) fn fetch_provider_archive(img: &str) -> Result<ProviderArchive> {
    let bytes = fetch_oci_bytes(img)?;
    ProviderArchive::try_load(&bytes)
        .map_err(|e| format!("Failed to load provider archive: {}", e).into())
}

fn invocation_from_callback(
    hostkey: &KeyPair,
    origin: &str,
    bd: &str,
    ns: &str,
    op: &str,
    payload: &[u8],
) -> Invocation {
    let binding = if bd.trim().is_empty() {
        // Some actor SDKs may not specify a binding field by default
        "default".to_string()
    } else {
        bd.to_string()
    };
    let target = if ns.len() == 56 && ns.starts_with("M") {
        WasccEntity::Actor(ns.to_string())
    } else {
        WasccEntity::Capability {
            binding,
            capid: ns.to_string(),
        }
    };
    Invocation::new(
        hostkey,
        WasccEntity::Actor(origin.to_string()),
        target,
        op,
        payload.to_vec(),
    )
}

pub(crate) fn gen_config_invocation(
    hostkey: &KeyPair,
    actor: &str,
    capid: &str,
    claims: Claims<wascap::jwt::Actor>,
    binding: String,
    values: HashMap<String, String>,
) -> Invocation {
    use wascc_codec::core::*;
    let mut values = values.clone();
    values.insert(
        CONFIG_WASCC_CLAIMS_ISSUER.to_string(),
        claims.issuer.to_string(),
    );
    values.insert(
        CONFIG_WASCC_CLAIMS_CAPABILITIES.to_string(),
        claims
            .metadata
            .as_ref()
            .unwrap()
            .caps
            .as_ref()
            .unwrap_or(&Vec::new())
            .join(","),
    );
    values.insert(CONFIG_WASCC_CLAIMS_NAME.to_string(), claims.name());
    values.insert(
        CONFIG_WASCC_CLAIMS_EXPIRES.to_string(),
        claims.expires.unwrap_or(0).to_string(),
    );
    values.insert(
        CONFIG_WASCC_CLAIMS_TAGS.to_string(),
        claims
            .metadata
            .as_ref()
            .unwrap()
            .tags
            .as_ref()
            .unwrap_or(&Vec::new())
            .join(","),
    );
    let cfgvals = CapabilityConfiguration {
        module: actor.to_string(),
        values,
    };
    let payload = serialize(&cfgvals).unwrap();
    Invocation::new(
        hostkey,
        WasccEntity::Actor(SYSTEM_ACTOR.to_string()),
        WasccEntity::Capability {
            capid: capid.to_string(),
            binding,
        },
        OP_BIND_ACTOR,
        payload,
    )
}

fn sha256_digest<R: Read>(mut reader: R) -> Result<Digest> {
    let mut context = Context::new(&SHA256);
    let mut buffer = [0; 1024];

    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        context.update(&buffer[..count]);
    }

    Ok(context.finish())
}

pub fn invocation_hash(target_url: &str, origin_url: &str, msg: &[u8]) -> String {
    use std::io::Write;
    let mut cleanbytes: Vec<u8> = Vec::new();
    cleanbytes.write(origin_url.as_bytes()).unwrap();
    cleanbytes.write(target_url.as_bytes()).unwrap();
    cleanbytes.write(msg).unwrap();
    let digest = sha256_digest(cleanbytes.as_slice()).unwrap();
    HEXUPPER.encode(digest.as_ref())
}

pub(crate) fn detect_core_host_labels() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert(
        CORELABEL_ARCH.to_string(),
        std::env::consts::ARCH.to_string(),
    );
    hm.insert(CORELABEL_OS.to_string(), std::env::consts::OS.to_string());
    hm.insert(
        CORELABEL_OSFAMILY.to_string(),
        std::env::consts::FAMILY.to_string(),
    );
    info!("Detected Intrinsic host labels. hostcore.arch = {}, hostcore.os = {}, hostcore.family = {}",
          std::env::consts::ARCH,
          std::env::consts::OS,
          std::env::consts::FAMILY,
    );
    hm
}

pub(crate) fn fetch_actor(actor_id: &str) -> Result<crate::actor::Actor> {
    let vec = crate::inthost::fetch_oci_bytes(actor_id)?;

    crate::actor::Actor::from_slice(&vec)
}

pub(crate) fn fetch_provider(
    provider_ref: &str,
    binding_name: &str,
    labels: Arc<RwLock<HashMap<String, String>>>,
) -> Result<(
    crate::capability::NativeCapability,
    Claims<wascap::jwt::CapabilityProvider>,
)> {
    use std::fs::File;
    use std::io::Write;

    let par = crate::inthost::fetch_provider_archive(provider_ref)?;
    let lock = labels.read().unwrap();
    let target = format!("{}-{}", lock[CORELABEL_ARCH], lock[CORELABEL_OS]);
    let v = par.target_bytes(&target);
    if let Some(v) = v {
        let path = std::env::temp_dir();
        let path = path.join(target);
        {
            let mut tf = File::create(&path)?;
            tf.write_all(&v)?;
        }
        let nc = NativeCapability::from_file(path, Some(binding_name.to_string()))?;
        if let Some(c) = par.claims() {
            Ok((nc, c))
        } else {
            Err(format!(
                "No embedded claims found in provider archive for {}",
                provider_ref
            )
            .into())
        }
    } else {
        Err(format!("No binary found in provider archive for {}", target).into())
    }
}

#[cfg(test)]
mod test {
    use super::Invocation;
    use crate::WasccEntity;
    use wascap::prelude::KeyPair;

    #[test]
    fn invocation_antiforgery() {
        let hostkey = KeyPair::new_server();
        // As soon as we create the invocation, the claims are baked and signed with the hash embedded.
        let inv = Invocation::new(
            &hostkey,
            WasccEntity::Actor("testing".into()),
            WasccEntity::Capability {
                capid: "wascc:messaging".into(),
                binding: "default".into(),
            },
            "OP_TESTING",
            vec![1, 2, 3, 4],
        );
        let res = inv.validate_antiforgery();
        println!("{:?}", res);
        // Obviously an invocation we just created should pass anti-forgery check
        assert!(inv.validate_antiforgery().is_ok());

        // Let's tamper with the invocation and we should hit the hash check first
        let mut bad_inv = inv.clone();
        bad_inv.target = WasccEntity::Actor("BADACTOR-EXFILTRATOR".into());
        assert!(bad_inv.validate_antiforgery().is_err());

        // Alter the payload and we should also hit the hash check
        let mut really_bad_inv = inv.clone();
        really_bad_inv.msg = vec![5, 4, 3, 2];
        assert!(really_bad_inv.validate_antiforgery().is_err());

        // And just to double-check the routing address
        assert_eq!(
            inv.target_url(),
            "wasmbus://wascc/messaging/default/OP_TESTING"
        );
    }
}
