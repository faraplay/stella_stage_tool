use std::{path::PathBuf, time::Instant};

use clap::{Parser, Subcommand};

mod build;
mod crypt;
mod csv;
mod dir;
mod extract;
mod inject;
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
    /// Extracts text from a file or directory of files.
    ///
    /// Currently supported file types: jxb, jxk
    ExtractText {
        /// Extract from all files in the specified directory instead.
        #[arg(short)]
        recursive: bool,
        /// The input file.
        in_path: PathBuf,
        /// The output file.
        out_path: PathBuf,
        /// Pattern to filter the extracted text with.
        ///
        /// If a filter is specified, only text from nodes whose name
        /// contains the filter string will be extracted.
        /// For example, setting the filter `-f jp` will extract text from
        /// nodes with name `jp` and `name_jp` but not `ch`.
        #[arg(short)]
        filter: Option<String>,
    },
    /// Injects text from a `csv` file into files.
    ///
    /// Currently supported file types: jxb, jxk
    InjectText {
        /// Inject into files in the specified directory instead.
        #[arg(short)]
        recursive: bool,
        /// The csv file containing the text to inject.
        csv_path: PathBuf,
        /// The file to inject text into.
        edit_path: PathBuf,
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
        Commands::ExtractText {
            recursive,
            in_path,
            out_path,
            filter,
        } => {
            if *recursive {
                extract::extract_text_directory(in_path, out_path, filter.as_deref())
                    .await
                    .expect("Error extracting text from files!");
                eprintln!("Extracted text from files in directory.")
            } else {
                let Some(extension) = in_path.extension() else {
                    panic!("File does not have an extension!");
                };
                if extension == "jxb" {
                    extract::extract_text_jxb_file(in_path, out_path, filter.as_deref())
                        .await
                        .expect("Error extracting text from jxb file!");
                } else if extension == "jxk" {
                    extract::extract_text_jxk_file(in_path, out_path, filter.as_deref())
                        .await
                        .expect("Error extracting text from jxk file!");
                } else {
                    panic!("Unsupported extension!");
                }
                eprintln!("Extracted text from file.");
            }
        }
        Commands::InjectText {
            recursive,
            csv_path,
            edit_path,
        } => {
            if *recursive {
                inject::inject_text_dir_files(csv_path, edit_path)
                    .await
                    .expect("Error injecting text into files in directory!");
                eprintln!("Injected text into files in directory.");
            } else {
                let Some(extension) = edit_path.extension() else {
                    panic!("File name of the file to edit does not have an extension!");
                };
                if extension == "jxb" {
                    inject::inject_text_jxb_file(csv_path, edit_path)
                        .await
                        .expect("Error injecting text into jxb file!");
                } else if extension == "jxk" {
                    inject::inject_text_jxk_file(csv_path, edit_path)
                        .await
                        .expect("Error injecting text into jxk file!");
                } else {
                    panic!("Unsupported extension!");
                }
                eprintln!("Injected text into file.");
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
