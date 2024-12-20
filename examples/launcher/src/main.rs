// Launcher used as docker entrypoint for servers.
// Edgegap allows you to modify the entrypoint only via API, not dashboard..
// but you can easily change ENV vars via the dashboard. So we read an ENV to decide
// which server binary to launch, which saves us a lot of configuration hassle.
// (all server binaries live in the same docker image)
use std::{
    env, fs,
    process::{exit, Command},
};

fn main() {
    // Read the environment variable EXAMPLE_NAME
    let example_name = env::var("EXAMPLE_NAME").unwrap_or_else(|_| {
        eprintln!("Environment variable EXAMPLE_NAME is not set.");
        exit(1);
    });

    // Get the directory of the currently running binary
    let current_exe = env::current_exe().unwrap_or_else(|err| {
        eprintln!("Failed to get current executable path: {}", err);
        exit(1);
    });
    let base_dir = current_exe.parent().unwrap_or_else(|| {
        eprintln!("Failed to determine the parent directory of the executable.");
        exit(1);
    });

    // Construct the directory and binary paths
    let dir_path = base_dir.join(&example_name);
    let binary_path = dir_path.join(&example_name);

    // println!("dir_path: {}", dir_path.display());
    // println!("binary_path: {}", binary_path.display());

    // Change to the directory
    if let Err(err) = env::set_current_dir(&dir_path) {
        eprintln!(
            "Failed to change directory to {}: {}",
            dir_path.display(),
            err
        );
        exit(1);
    }

    // Ensure the binary exists
    if !fs::metadata(&binary_path)
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        eprintln!("Binary not found at {}", binary_path.display());
        exit(1);
    }

    println!("Launching {example_name} server...");
    // Execute the binary with argument "server"
    let status = Command::new(&binary_path)
        .arg("server")
        .status()
        .unwrap_or_else(|err| {
            eprintln!("Failed to execute {}: {}", binary_path.display(), err);
            exit(1);
        });

    if !status.success() {
        eprintln!("{example_name} server exited with status: {}", status);
        exit(status.code().unwrap_or(666));
    }
}
