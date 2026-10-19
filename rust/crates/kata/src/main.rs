mod cli;
mod clock;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Err(error) = cli::run(&args) {
        eprintln!(
            "kata: {error}\nUse --help for commands. Check the reference, config, and state path."
        );
        std::process::exit(1);
    }
}
