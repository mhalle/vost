use std::process;

fn main() {
    if let Err(e) = vost::cli::run() {
        eprintln!("error: {}", e);
        process::exit(1);
    }
}
