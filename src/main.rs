mod ipc;
mod markdown;
mod ui;
mod vault;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        ui::run();
        return;
    }
    if let Err(e) = ipc::cli(&args) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
