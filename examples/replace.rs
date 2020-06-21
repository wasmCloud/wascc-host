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
use std::io;
use wascc_host::{Actor, NativeCapability, WasccHost};

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let host = WasccHost::new();
    host.add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        None,
    )?)?;

    host.bind_actor(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:keyvalue",
        None,
        redis_config(),
    )?;
    host.bind_actor(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:http_server",
        None,
        http_config(),
    )?;

    println!(
        "**> curl localhost:8081/counter1 to test, then press ENTER to replace with a new version"
    );
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    // Note that we don't supply the public key of the one to replace, it
    // comes straight from the Actor being replaced. You cannot replace actors
    // that do not have the same public key as a security measure against
    // malicious code
    host.replace_actor(Actor::from_file(
        "./examples/.assets/kvcounter_tweaked.wasm",
    )?)?;
    println!("**> KV counter replaced, issue query to see the new module running.");

    println!("**> Press ENTER to remove the key-value provider");
    io::stdin().read_line(&mut input)?;
    host.remove_native_capability("wascc:keyvalue", None)?;

    println!("**> Press ENTER to add an in-memory key-value provider");
    io::stdin().read_line(&mut input)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libkeyvalue.so",
        None,
    )?)?;

    println!("**> Now your counter should have started over.");

    std::thread::park();

    Ok(())
}

fn redis_config() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("URL".to_string(), "redis://127.0.0.1:6379".to_string());

    hm
}

fn http_config() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), "8081".to_string());

    hm
}
