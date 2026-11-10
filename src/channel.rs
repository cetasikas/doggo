//! Doggo is a personal Wisp fork with an isolated runtime identity.
use windows::core::{PCWSTR, w};
#[allow(dead_code)] // Used by the settings companion.
pub const MANUAL_UPDATES: bool = true;
pub const NAME: &str = "Doggo";
pub const DATA_DIR: &str = "doggo";
pub const LAUNCHER_CLASS: PCWSTR = w!("Doggo.Launcher");
pub const INSTANCE_MUTEX: PCWSTR = w!(r"Local\Doggo.Launcher.SingleInstance");
pub const RUN_VALUE: PCWSTR = w!("Doggo");

#[cfg(test)]
mod tests {
    #[test]
    fn fork_identity_is_isolated() {
        assert_eq!(super::DATA_DIR, "doggo");
        assert_eq!(super::NAME, "Doggo");
        assert_eq!(unsafe { super::LAUNCHER_CLASS.to_string().unwrap() }, "Doggo.Launcher");
        assert_eq!(unsafe { super::RUN_VALUE.to_string().unwrap() }, "Doggo");
    }
}
