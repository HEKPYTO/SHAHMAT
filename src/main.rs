use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--health-check") {
        println!("ok");
        return ExitCode::from(0);
    }
    if args.len() > 1 && args[1] == "perft" {
        eprintln!("perft movegen lands in Phase 2");
        return ExitCode::from(2);
    }
    eprintln!("usage: shahmat-svc --health-check | perft <fen|startpos> <depth> [--no-bulk] [--divide]");
    ExitCode::from(2)
}
