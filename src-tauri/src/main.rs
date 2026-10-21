// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(kibo_agent_lib::meeting_sidecar::FLAG) {
        std::process::exit(kibo_agent_lib::meeting_sidecar::main(&args[1..]));
    }
    kibo_agent_lib::run()
}
