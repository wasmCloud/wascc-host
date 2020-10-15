# waSCC Host Change Log

All notable changes to this project will be documented in this file.

_The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)_

## [0.14.0] - 2020 OCT 30

This version corresponds to the project milestone [0.14](https://github.com/wascc/wascc-host/milestone/3)

### Added

* Host runtime will now look at the `OCI_REGISTRY_USER` and `OCI_REGISTRY_PASSWORD` environment variables whenever it needs to contact an OCI registry.
* Added the `add_native_capability_from_registry` to download and load an OS/CPU arch appropriate provider plugin.

### Changed

* All non-local references to capability providers and actors are now assumed to be registry references that can be satisfied by the indicated OCI registry. If basic authentication is required by that registry, then you can supply those credentials via the `OCI_REGISTRY_USER` and `OCI_REGISTRY_PASSWORD` environment variables.
* The `add_actor_from_gantry` function has been renamed to `add_actor_from_registry`
* Manifest files will now load either actors (if the actor string exists as a file) or registry-hosted modules (if the actor string is a registry reference)

### Caution

The ability to remotely schedule capability providers in this version should be considered unstable, to be 
stabilized in the 0.15.0 release (the "async rewrite"), which will include a rework of how capability providers
internally subscribe to the message bus and identify themselves to the host and lattice.

## [0.13.0] - 2020 SEP 30

This version corresponds to the project milestone [0.13](https://github.com/wascc/wascc-host/milestone/2)

### Added

* _Lattice Control Plane Protocol_ - A new protocol is now supported on the lattice to allow for "auctions" to take place to gather a list of hosts willing to launch a given actor. An auction request is sent out with a set of constraints, any host that can satisfy those contraints responds affirmatively to the auction, and then the conductor of the auction chooses from among the responding hosts and sends a command to that specific host telling it to launch the actor in question. Launching that actor is then done by downloading the actor's bytes from a [Gantry](https://github.com/wascc/gantry) server and running it. You can specify a set of key/value pairs that define constraints that are matched against host labels to definite affinity-style scheduling rules.
* _Sim Gantry_ - The gantry repo now has a "sim gantry" binary that developers can use to instantly start a Gantry host atop an arbitrary directory that contains signed actor modules.
* _Lattice-Scoped Bindings_ - In lattice mode, calling `set_binding` will tell all running instances of a given capability provider to provision resources for the given actor+configuration. If a capability provider is added to a completely empty host, it will re-establish its bindings (including provisioning resources) by querying the lattice. If all instances of that provider are shut down, then the lattice will "forget" those bindings. Actors can be added and removed without the need to re-bind them to their capability providers.
* There is now a `remove_binding` function on the `Host` struct that will tell all running instances of a given capability provider to de-provision resources for a given actor group.
* waPC now supports the choice of multiple underlying WebAssembly engines or drivers, so waSCC host now supports them through the use of feature flags. You can now choose your engine between `wasmtime` (the default) and `wasm3`. For more information on the difference, please see the driver repository in the [waPC](https://github.com/wapc) Github organization.

### Changed

* The `bind_actor` function has been renamed to `set_binding` to better provide context around the distributed, idempotent nature of bindings.
* You can no longer invoke `configure_gantry` on a running host. The gantry client can only be supplied by the host builder now, further improving the runtime security and reliability of the host.
* The `gantry` feature has been merged with the `lattice` feature. Gantry (client) support is no longer an isolated opt-in feature because both require the NATS connection in order to function and it didn't make sense to communicate with Gantry while not having the ability to connect to a lattice.
* Renamed `from_bytes` on the `Actor` struct to `from_slice`, and it now takes a `&[u8]` parameter instead of an owned vector.
* Loading an actor from gantry via the host API call now requires a revision number (0 indicates pull the latest)
* The Gantry client API to download an actor no longer requires a callback, it simply returns a vector of bytes.

### Removed

* You can no longer call `set_label` on a running host. Labels _must_ be set with a `HostBuilder`

## [0.12.0] - 2020 AUG 30

This version corresponds to the project milestone 0.12

### Added

* _Host Labels_ - You can now add arbitrary key/value pairs to a host manifest (when feature enabled) or via the `set_label` function (when `lattice` is enabled). These labels are discoverable via lattice host probe query. The host runtime will automatically add the appropriate values for the following reserved labels which cannot be overridden:

    * hostcore.os
    * hostcore.osfamily
    * hostcore.arch

* _Uptime_ - Uptime is now being tracked when the `lattice` feature is enabled, and will be reported in response to host probes on the control subject.
* _Bus Events_ - When compiled in lattice mode, the waSCC host will emit events on the lattice that allow changes in state (e.g. actor start, stop, provider load, unload, etc) to be monitored either directly via NATS or with a command-line utility like `latticectl`
* _Lattice Namespaces_ - You can now provide a namespace as a unit of isolation. This allows you to run multiple hosts on multiple isolated lattices all on top of the same NATS infrastructure. This can be done through the `LATTICE_NAMESPACE` environment variable or explicitly through the new `HostBuilder`
* _HostBuilder_ - A new builder syntax is available to more fluently configure a new host.

### Changed

* The `wascc_host` binary will now default its log level to `INFO`, and you can override this behavior with the standard `RUST_LOG` environment variable syntax.
* The crate's `Error` type now requires `Send` and `Sync`. This should have very little impact on consumers.
* The `WasccHost` struct has been renamed to `Host`. Per Rust style guidelines, structs should not be prefixed with their module names.

## [0.11.0] - 2020 JUL 24

This version lays the groundwork for providing more advanced and pluggable authorization functionality in the future.

### Removed

* The `set_auth_hook` function has been removed from the `WasccHost` struct

### Added

* The `with_authorizer` function has been added to `WasccHost` to allow a developer to create an instance of a waSCC host with a custom authorizer (anything that implements the new `Authorizer` trait). While waSCC host will _always_ enforce the core capability claims when validating whether an actor can communicate with a given capability provider, the new `Authorizer` allows developers to build custom code that further constrains / limits the authorization when loading actors into a host and when actors are attempting to invoke operations against other actors or capability providers.

## [0.10.0] - 2020 JUL 9

This release includes several lattice-related enhancements as well as some security and stability improvements.

### Changed

* The `Invocation` type now includes its own set of claims that must be verified by receiving code. This prevents invocations from being forged on the wire in the case of intrusion.
* The `InvocationTarget` enum has been renamed to `WasccEntity` to better clarify the expected communications patterns
* Middleware now has the ability to indicate a stop or a short-circuit in the middleware change. The trait signature for middleware has changed and any middleware structs built against 0.9.0 will have to be upgraded.

### Added

* Each waSCC host instance now generates its own unique signing key (of type server, nkey prefix is `N` for "node"). This signing key is used to mint forge-proof invocations for transmission over the lattice.
* The host now announces (at `info` level) to the stdout log its version number.
* All waSCC hosts in lattice mode will now perform an antiforgery check on inbound invocations.
* All waSCC hosts in lattice mode will now respond to inventory requests allowing authorized clients to probe the lattice for actors, capabilities, bindings, and hosts.
* The waSCC host will now supply a number of additional actor claims (name, capabilities, tags, expiration, and issuer) to the capability provider during the binding in the form of custom key-value pairs added to the configuration hash map. For the list of these new keys, see [waSCC Codec](../wascc-codec).

## [0.8.0] - 2020 JUN 8

This release was primarily to accomodate the upgrade to the newest version of the [waSCC Codec](../wascc-codec).

### Changed

All capability providers (including _portable_ WASI providers) are now required to respond to the operation `OP_GET_CAPABILITY_DESCRIPTOR` and return a messagepack-serialized struct containing metadata about the capability provider. This metadata includes:

* Name
* Documentation description
* Version (semver string) and Revision (monotonic)
* List of supported operations

We created a simple _builder_ syntax that makes it easy and readable for capability providers to supply a capability descriptor:

```rust
/// Obtains the capability provider descriptor
fn get_descriptor(&self) -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(serialize(
        CapabilityDescriptor::builder()
            .id(CAPABILITY_ID)
            .name("Default waSCC HTTP Server Provider (Actix)")
            .long_description("A fast, multi-threaded HTTP server for waSCC actors")
            .version(VERSION)
            .revision(REVISION)
            .with_operation(
                OP_HANDLE_REQUEST,
                OperationDirection::ToActor,
                "Delivers an HTTP request to an actor and expects an HTTP response in return",
            )
            .build(),
    )?)
}
```

**NOTE** - This is a breaking change, so old versions of capability providers will _not_ work with this version of the waSCC host.
