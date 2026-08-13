use std::{path::PathBuf, time::Instant};

use clap::{Parser, Subcommand};

mod crypt;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Decrypts a file or directory of files.
    Decrypt {
        /// Decrypt all files in the specified directory instead.
        #[arg(short)]
        recursive: bool,
        /// The input file or directory.
        in_path: PathBuf,
        /// The output file or directory.
        out_path: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Decrypt {
            recursive,
            in_path,
            out_path,
        } => {
            let now = Instant::now();
            if *recursive {
                crypt::decrypt_directory(in_path, out_path).expect("Failed to decrypt all files!");
                eprintln!("Decrypted all files.")
            } else {
                crypt::decrypt_file(in_path, out_path).expect("Failed to decrypt file!");
                eprintln!("Decrypted file.")
            }
            let elapsed = now.elapsed();
            eprintln!("Time elapsed: {} seconds.", elapsed.as_secs_f32());
        }
    }
}
