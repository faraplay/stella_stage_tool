use std::{path::PathBuf, time::Instant};

use clap::{Parser, Subcommand};
use tokio::task::JoinSet;

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

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Decrypt {
            recursive,
            in_path,
            out_path,
        } => {
            let now = Instant::now();
            if *recursive {
                let mut set = JoinSet::new();
                crypt::decrypt_directory(in_path, out_path, &mut set)
                    .await
                    .expect("Failed to read directory contents!");
                set.join_all().await;
                eprintln!("Decrypted all files.")
            } else {
                crypt::decrypt_file(in_path, out_path)
                    .await
                    .expect("Failed to decrypt file!");
                eprintln!("Decrypted file.")
            }
            let elapsed = now.elapsed();
            eprintln!("Time elapsed: {} seconds.", elapsed.as_secs_f32());
        }
    }
}
