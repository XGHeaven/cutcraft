use clap::Parser;
use cutcraft::app::{self, Cli};

fn main() {
    let cli = Cli::parse();
    let doctor = matches!(cli.command, app::Cmd::Doctor { .. });
    let result = app::dispatch(cli);
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
