use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut bulk = true;
    let mut divide = false;
    let mut pos = 0;
    for a in &args[1..] {
        match a.as_str() {
            "--no-bulk" => bulk = false,
            "--divide" => divide = true,
            _ => {
                if pos == 0 {
                    pos += 1;
                } else {
                    pos += 1;
                }
            }
        }
    }
    println!("shahmat perft gate stub bulk={bulk} divide={divide} pos_args={pos}");
    println!("full movegen lands in Phase 2");
}
