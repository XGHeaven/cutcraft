use clap::Parser;
use cutcraft::app::{self, Cli};

fn main() {
    let cli = Cli::parse();
    let doctor = matches!(cli.command, app::Cmd::Doctor { .. });
    let result = isolate_stdio_inheritance().and_then(|()| app::dispatch(cli));
    match result {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value).unwrap());
            if doctor && value["ok"] == false {
                std::process::exit(1);
            }
        }
        Err(error) => {
            let mut detail =
                if let Some(remote) = error.downcast_ref::<cutcraft::daemon::RemoteError>() {
                    remote.detail.clone()
                } else {
                    serde_json::json!({"message":format!("{error:#}")})
                };
            if let Some(tool) = error.downcast_ref::<cutcraft::media_process::MediaToolError>() {
                detail["code"] = serde_json::json!(tool.code());
                detail["executable"] = serde_json::json!(tool.executable);
                detail["hint"] = serde_json::json!(tool.hint());
            }
            eprintln!("{}", serde_json::json!({"ok":false,"error":detail}));
            std::process::exit(1);
        }
    }
}

// Rust 1.86 CreateProcess inherits unrelated inheritable handles. Clear the
// original standard handles before any child is started: Command still makes
// inheritable duplicates for the stdio explicitly configured on each child.
// Otherwise a detached editor can retain the caller's output pipe indefinitely.
fn isolate_stdio_inheritance() -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetStdHandle(kind: u32) -> *mut c_void;
            fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
        }
        for kind in [-10i32, -11, -12] {
            // SAFETY: GetStdHandle returns a borrowed process handle; it is not
            // closed here. HANDLE_FLAG_INHERIT is the only flag being changed.
            unsafe {
                let handle = GetStdHandle(kind as u32);
                if !handle.is_null()
                    && handle as isize != -1
                    && SetHandleInformation(handle, 1, 0) == 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
        }
    }
    Ok(())
}
