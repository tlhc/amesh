mod bridge;
mod cli;
mod hub;
mod tui;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("bridge") {
        if let Err(error) = bridge::run(&args[1..]).await {
            eprintln!("amesh: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(code) = cli::run() {
        std::process::exit(code);
    }
    if let Err(error) = hub::serve().await {
        eprintln!("amesh: {error}");
        std::process::exit(1);
    }
}
