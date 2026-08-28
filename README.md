# stella_stage_tool

This is a tool for working with game files from the game iDOLM@STER Stella Stage.

## How to use

### File decryption

First you will want to decrypt the game files. You can decrypt an individual file with the `decrypt` command, e.g. running

```sh
stella_stage_tool decrypt asset/message/message_jp.jxb decrypted/message_jp.jxb
```

will take the file at `asset/message/message_jp.jxb` and output the decrypted file in the folder `decrypted` with filename `message_jp.jxb`.

You can decrypt all files in a folder instead with the `-r` flag, e.g.

```sh
stella_stage_tool decrypt -r asset/gscript/ex2 decrypted/ex2
```

will decrypt every file in `asset/gscript/ex2` and output the decrypted files in `decrypted/ex2`,
using the same subfolder structure.

### File encryption

You can re-encrypt game files using the `encrypt` command, in essentially the same way as the `decrypt` command.
For example, running

```sh
stella_stage_tool encrypt decrypted/message_jp.jxb encrypted/message_jp.jxb
```

will take the file at `decrypted/message_jp.jxb` and output the encrypted file at `encrypted/message_jp.jxb`.

Running

```sh
stella_stage_tool encrypt -r decrypted/ex2 encrypted/ex2
```

will encrypt every file in `decrypted/ex2` and output the encrypted files in `encrypted/ex2` using the same subfolder structure.

The tool uses a fast compression algorithm by default. If you want to compress the files as small as possible when encrypting, you can use the `-s` flag.

### Text injection

You can inject text from `csv` files into decrypted `jxb` and `jxk` files with the `inject-text` command.
For example, running

```sh
stella_stage_tool inject-text message_jp.csv message_jp.jxb
```

will inject all text in the `message_jp.csv` spreadsheet into the file `message_jp.jxb`.
**Note that this overwrites the file you are injecting text into!**

You can also inject text from a `csv` file into multiple `jxb`/`jxk` files in the same folder by using the `-r` flag.
For example, running

```sh
stella_stage_tool inject-text -r har.csv decrypted/ex2/har
```

will inject text from `csv` into the files in the folder `decrypted/ex2/har`, using the filename in the first column of the `csv` to determine which file to inject each line into. Once again, note that **this overwrites the files you are injecting text into!**

## Other commands

The tool also has `extract`, `extract-text` and `build` commands. For more information on their syntax, you can run `stella_stage_tool help`.

## AI Disclaimer

Generative AI was used in the reverse-engineering of the game's decryption algorithm.

However **all of the code in this repository was written without the use of generative AI.**

## Acknowledgements

- File decryption is based on the [tools](https://reshax.com/files/file/2788-idolmster-ps4-idolmaster-platinum-stars-stella-stage-tools/) by **daemon**
