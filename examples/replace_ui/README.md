# Replace GUI

This is a UI designed to be used while running the replace example. To start the replace example back-end, run the following from the `wascc-host` root folder:

```shell
RUST_LOG=info,cranelift_wasm=warn cargo run --example replace
```

For this back end, you will need `Redis` installed to demonstrate one of the two key-value store capability providers.

Then, start the UI:

```shell
./start.sh
```

Note that you will need `nginx` installed and you might need to  `sudo` the start script if you run into permissions problems writing to the nginx log directory.
