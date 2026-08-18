//! shrekctl — operator CLI.
//!
//! Phase-1 scaffold: prints planned surface and exits. No subcommand does anything yet.

fn main() {
    eprintln!("shrekctl {} — Phase-1 scaffold (no subcommands implemented)", env!("CARGO_PKG_VERSION"));
    eprintln!("planned surface:");
    eprintln!("  shrek find | history <path> | related <path> | status      (swamp — Phase 6+)");
    eprintln!("  shrek run --trust=<tier> --caps=<profile> <cmd>             (isolation — Phase 5)");
    eprintln!("  shrek audit --agent <id>                                    (provenance — Phase 8)");
    std::process::exit(0);
}
