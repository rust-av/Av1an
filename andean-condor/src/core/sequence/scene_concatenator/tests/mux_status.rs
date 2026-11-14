#[cfg(target_os = "linux")]
use super::*;
#[cfg(target_os = "linux")]
use crate::utils::cancellation::Cancelled;

#[cfg(target_os = "linux")]
#[path = "../../../../../tests/core/support/process/wait.rs"]
mod wait;
#[cfg(target_os = "linux")]
use wait::wait_with_timeout;

#[cfg(target_os = "linux")]
#[path = "../../../../../tests/core/sequence/scene_concatenator/fixtures/mux_status.rs"]
mod fixture;

#[cfg(target_os = "linux")]
mod internal_integration {
    use super::*;

    #[test]
    fn mkvmerge_force_before_commit_withdraws_owned_output_but_preserves_existing_destination() {
        fixture::isolated(
            module_path!(),
            "mkvmerge_force_before_commit_withdraws_owned_output_but_preserves_existing_destination",
            &[("final", "1", "valid", "force"), ("final", "1", "preserve", "force-existing")],
        );
    }
}
