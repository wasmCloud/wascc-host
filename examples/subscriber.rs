// Copyright 2015-2020 Capital One Services, LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use wascc_host::{Actor, NativeCapability, WasccHost};

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let host = WasccHost::new();
    host.add_actor(Actor::from_file("./examples/.assets/subscriber.wasm")?)?;
    host.add_actor(Actor::from_file("./examples/.assets/subscriber2.wasm")?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_nats.so",
        None,
    )?)?;

    host.bind_actor(
        "MBHRSJORBXAPRCALK6EKOBBCNAPMRTM6ODLXNLOV5TKPDMPXMTCMR4DW",
        "wascc:messaging",
        None,
        generate_config("test"),
    )?;

    host.bind_actor(
        "MDJUPIQFWEHWE4XHPWHOJLW42SJDPVBQVDC2NV3T3O4ELXXVOXLA5M4I",
        "wascc:messaging",
        None,
        generate_config("second_test"),
    )?;
    std::thread::park();

    Ok(())
}

fn generate_config(sub: &str) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("SUBSCRIPTION".to_string(), sub.to_string());
    hm.insert("URL".to_string(), "nats://localhost:4222".to_string());

    hm
}
