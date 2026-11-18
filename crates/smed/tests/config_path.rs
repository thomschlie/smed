//! `config::default_path` reads `XDG_CONFIG_HOME` and `HOME`. Changing the environment is only
//! sound while no other thread reads it, so this file (its own test process) holds a single test.

use std::path::PathBuf;

use smed::config::default_path;

#[test]
fn the_config_lives_in_the_xdg_config_dir_else_in_home() {
    // SAFETY: the only test in this process, and it starts no threads.
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", "/xdg");
        std::env::set_var("HOME", "/home/user");
    }
    assert_eq!(default_path(), Some(PathBuf::from("/xdg/smed/config.toml")));

    // SAFETY: as above.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", "") };
    assert_eq!(
        default_path(),
        Some(PathBuf::from("/home/user/.config/smed/config.toml")),
        "XDG_CONFIG_HOME empty"
    );

    // SAFETY: as above.
    unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    assert_eq!(
        default_path(),
        Some(PathBuf::from("/home/user/.config/smed/config.toml")),
        "XDG_CONFIG_HOME unset"
    );

    // SAFETY: as above.
    unsafe { std::env::set_var("HOME", "") };
    assert_eq!(default_path(), None, "HOME empty");

    // SAFETY: as above.
    unsafe { std::env::remove_var("HOME") };
    assert_eq!(default_path(), None, "neither set");
}
