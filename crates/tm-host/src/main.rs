//! `tm-host`: the standalone host binary (no GUI dependencies). The GUI can also act as a host
//! itself with `terminal-manager --host`.

use tm_core::paths::HostMode;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None | Some("run") => tm_host::run(HostMode::User, true),
        Some("-h" | "--help" | "help") => {
            println!("Usage: tm-host [run]\n\n  run   Run a host for the current user in the foreground (default)");
            return;
        }
        Some(other) => {
            eprintln!("Unknown command `{other}`. Try `tm-host --help`.");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("tm-host: {e}");
        std::process::exit(1);
    }
}
