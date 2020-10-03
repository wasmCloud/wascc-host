extern crate wascc_actor as actor;

use actor::prelude::*;

actor_handlers! {
    codec::core::OP_INITIALIZE => handle_config
}

fn handle_config(_msg: codec::core::CapabilityConfiguration) -> HandlerResult<()> {
    let blob_store = actor::objectstore::host("fs_host_error_test_binding");

    let res = blob_store.remove_container(
        "very_long_name_that_wont_be_present_and_should_result_in_a_spectacular_error",
    );

    match res {
        Ok(_) => panic!(
            "Expected the 'blob_store.remove_container' call to return an error, did api change??"
        ),
        Err(_e) => Err("THIS IS THE WAY".into()),
    }
}
