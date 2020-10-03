extern crate wascc_actor as actor;

use actor::prelude::*;

actor_handlers! {
    codec::core::OP_INITIALIZE => handle_config
}

fn handle_config(_msg: codec::core::CapabilityConfiguration) -> HandlerResult<()> {
    let blob_store = actor::objectstore::host("fs_host_error_test_binding");

    let res = blob_store.remove_container("dummy_container_removal");

    match res {
        Ok(_) => panic!(
            "Expected the 'blob_store.remove_container' call to return an error, did api change??"
        ),
        Err(e) => Err(format!("{}: THIS IS THE WAY", e).into()),
    }
}
