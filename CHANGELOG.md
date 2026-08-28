# Version 0.5.0

Added:

- `inject-text` command that injects text from a `csv` file into `jxb` and `jxk` files

# Version 0.4.0

Added:

- `extract-text` command that extracts text from `jxb` and `jxk` files
    - `-f` filter option to only extract text in a specific language

# Version 0.3.2

Changed:

- More accurate `jxb`/`jxk` extraction algorithm that works on more files

# Version 0.3.1

Changed:

- `encrypt` command now produces files with the correct checksum, meaning the game can now read it correctly
- `decrypt` command now checks the file checksum

# Version 0.3.0

Added:

- `build` command that builds a `jxb` file from a `xml` file
- The `build` command can also build a `jxk` file from a directory containing a `info.xml` file

# Version 0.2.0

Added:

- `extract` command that extracts files/data from `jxk` and `jxb` files

# Version 0.1.0

Added:

- `decrypt` command that decrypts game files
- `encrypt` command that encrypts game files
