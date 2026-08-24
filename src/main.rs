use std::{path::PathBuf, time::Instant};

use clap::{Parser, Subcommand};

mod build;
mod crypt;
mod dir;
mod extract;
mod jxb;
mod jxk;
mod semaphore;
mod size;

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
    /// Extracts a file or directory of files.
    ///
    /// Currently supported file types: jxb, jxk
    Extract {
        /// Extract all files in the specified directory instead.
        #[arg(short)]
        recursive: bool,
        /// The input file.
        in_path: PathBuf,
        /// The output file.
        out_path: PathBuf,
    },
    /// Builds a file from a given file or directory of files.
    ///
    /// Currently supported file types: jxb, jxk
    /// - To build a jxb file, the input should be a xml file.
    /// - To build a jxk file, the input should be a directory containing a file called 'info.xml'.
    Build {
        /// The input file.
        in_path: PathBuf,
        /// The output file.
        out_path: PathBuf,
    },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let cli = Cli::parse();

    let now = Instant::now();
    match &cli.command {
        Commands::Decrypt {
            recursive,
            in_path,
            out_path,
        } => {
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
        }
        Commands::Encrypt {
            recursive,
            small,
            in_path,
            out_path,
        } => {
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
        }
        Commands::Extract {
            recursive,
            in_path,
            out_path,
        } => {
            if *recursive {
                extract::extract_directory(in_path, out_path)
                    .await
                    .expect("Error extracting files!");
                eprintln!("Extracted files in directory.")
            } else {
                let Some(extension) = in_path.extension() else {
                    panic!("File does not have an extension!");
                };
                if extension == "jxb" {
                    extract::extract_jxb_file(in_path, out_path)
                        .await
                        .expect("Error extracting jxb file!");
                } else if extension == "jxk" {
                    extract::extract_jxk_file(in_path, out_path)
                        .await
                        .expect("Error extracting jxk file!");
                } else {
                    panic!("Unsupported extension!");
                }
                eprintln!("Extracted file.");
            }
        }
        Commands::Build { in_path, out_path } => {
            let Some(extension) = out_path.extension() else {
                panic!("Output file name does not have an extension!");
            };
            if extension == "jxb" {
                build::build_jxb_file(in_path, out_path)
                    .await
                    .expect("Error building jxb file!");
            } else if extension == "jxk" {
                build::build_jxk_file(in_path, out_path)
                    .await
                    .expect("Error building jxk file!");
            } else {
                panic!("Unsupported extension!");
            }
            eprintln!("Built file.");
        }
    }
    let elapsed = now.elapsed();
    eprintln!("Time elapsed: {} seconds.", elapsed.as_secs_f32());
}
