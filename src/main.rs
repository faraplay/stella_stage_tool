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
    /// Encrypts a file or directory of files.
    Encrypt {
        /// Encrypt all files in the specified directory instead.
        #[arg(short)]
        recursive: bool,
        /// Compress files as small as possible instead of optimising for speed.
        #[arg(short)]
        small: bool,
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
                crypt::decrypt_directory(in_path, out_path)
                    .await
                    .expect("Failed to decrypt files in directory!");
                eprintln!("Decrypted files in directory.")
            } else {
                crypt::decrypt_file(in_path, out_path)
                    .await
                    .expect("Failed to decrypt file!");
                eprintln!("Decrypted file.")
            }
            let elapsed = now.elapsed();
            eprintln!("Time elapsed: {} seconds.", elapsed.as_secs_f32());
        }
        Commands::Encrypt {
            recursive,
            small,
            in_path,
            out_path,
        } => {
            let now = Instant::now();
            if *recursive {
                crypt::encrypt_directory(in_path, out_path, *small)
                    .await
                    .expect("Failed to encrypt files in directory!");
                eprintln!("Encrypted files in directory.")
            } else {
                crypt::encrypt_file(in_path, out_path, *small)
                    .await
                    .expect("Failed to encrypt file!");
                eprintln!("Encrypted file.")
            }
            let elapsed = now.elapsed();
            eprintln!("Time elapsed: {} seconds.", elapsed.as_secs_f32());
        }
    }
}
