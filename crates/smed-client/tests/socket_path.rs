//! `default_socket_path` reads `XDG_RUNTIME_DIR`. Changing the environment is only sound while
//! no other thread reads it, so this file (its own test process) holds a single test.

use std::path::PathBuf;

use smed_client::default_socket_path;

#[test]
fn the_socket_lives_in_the_runtime_dir_when_there_is_one() {
    // SAFETY: the only test in this process, and it starts no threads.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000") };
    assert_eq!(
        default_socket_path(),
        Some(PathBuf::from("/run/user/1000/smed/api.sock"))
    );

    // SAFETY: as above.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", "") };
    assert_eq!(default_socket_path(), None, "empty");

    // SAFETY: as above.
    unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
    assert_eq!(default_socket_path(), None, "unset");
}
