[![crates.io](https://img.shields.io/crates/v/wascc-host.svg)](https://crates.io/crates/wascc-host)&nbsp;
![Rust build](https://github.com/wascc/wascc-host/workflows/Rust/badge.svg)&nbsp;
![license](https://img.shields.io/crates/l/wascc-host.svg)&nbsp;
[![documentation](https://docs.rs/wascc-host/badge.svg)](https://docs.rs/wascc-host)

# waSCC Host

The _WebAssembly Secure Capabilities Connector_ (waSCC) host library allows consumers to add actor modules, portable capability providers, and native capability providers to a single runtime host and provide each of those modules with their own unique, per-capability configuration. This allows actors to each have their own separate message broker subscriptions, key-value stores, HTTP server ports, etc.

For more information on concepts, architecture, and tutorials, check out [wascc.dev](https://wascc.dev).

## Tutorials

- [Build your first actor](https://wascc.dev/tutorials/first-actor/)
- [Create a capability provider](https://wascc.dev/tutorials/native-provider/)

## Building the `wascc-host` binary

To build the optional `wascc-host` binary, run the following command from the project root:

```
$ cargo build --bin wascc-host --features "bin manifest"
```

## Running the examples

To run the examples, issue the following command from the root `wascc-host` directory:

```
$ cargo run --example [example name]
```

Where the example name is the name (without the `.rs`) of any of the examples in the examples folder. The examples use `env_logger` so you can adjust log levels with the `RUST_LOG` environment variable

e.g.

```
$ RUST_LOG=wascc_host=trace cargo run --example kvcounter
```

## Prerequisites

- The latest version of Rust.
- For some examples (subscriber, lattice, et al), you will need an instance of [NATS](https://nats.io) running locally.
- For some examples (kvcounter, replace, start_stop, et al), you will need [Redis](https://redis.io/) running locally on the default port.
- For the [`lattice`](https://wascc.dev/docs/lattice/overview/) examples, in addition to the local [NATS](https://nats.io) server you will need to build `wascc-host` with the `lattice` feature.

**NOTE** - All of the rust examples use _native_ capability providers, and therefore utilize the linux dynamic libraries (`.so` files). To use these examples on a Mac, you will need to manully build Mac dynamic libraries (`.dylib` files) and modify the examples to read those files instead.

## FAQ

### The examples don't do anything

Some examples don't produce interesting output by default, they create servers or perform other invisible tasks. Try increasing the log output by setting the `RUST_LOG` environment variable (e.g. `RUST_LOG=wascc_host cargo run ...`) and tailing logs for related services like redis or nats-server.

### I'm getting the error: "no suitable image found"

The pre-compiled libraries included in `./examples/.assets` don't work for your system. See the note in [prerequisites](#prerequisites). You will need to clone the repository for the associated provider and compile it manually. For example, to build a compatible version of "libwascc_httpsrv.so" on a Mac:

- clone [git@github.com:wascc/http-server-provider.git](https://github.com/wascc/http-server-provider)
- run `cargo build` in the cloned repository
- modify the example, replacing `"libwascc_httpsrv.so"` with `"./http-server-provider/target/debug/libwascc_httpsrv.dylib"`

### The lattice example fails with "waSCC Host Error: Attempted bus call for wasmbus.provider.wascc.http_server.default with no subscribers"

You need to build `wascc-host` with the `lattice` feature:

```
$ cargo build --bin wascc-host --features "bin manifest lattice"
```

### I'm getting the error: "Failed to establish TCP connection: Connection refused"

You are probably trying to run an example that depends on [NATS](https://nats.io) without a local nats-server. Check that you have `nats-server` running on the default port or change the example to point to a running NATS server.

### The redis example returns "Failed to handle request"

The example can not connect to a redis server. Ensure your local server is running on the default port or change the example source to point to a running redis server.

### I'm getting an error: target `[example]` in package `wascc-host` requires the features: `[feature]`

Some examples require optional features of `wascc-host`. Enable the specified feature to proceed, e.g. `cargo run --example kvcounter_manifest --features=manifest`. Cargo's output will give you the exact flag to add.

## waSCC on Kubernetes

Looking to deploy waSCC actors on Kubernetes? Check out the [krustlet](https://github.com/deislabs/krustlet) project.
