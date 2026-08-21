use binrw::BinWrite;
use std::io::{Seek, Write};

struct Position {
    pos: u64,
}

impl Write for Position {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let count = buf.len();
        self.pos += count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for Position {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match pos {
            std::io::SeekFrom::Start(pos) => self.pos = pos,
            std::io::SeekFrom::End(_) => panic!(),
            std::io::SeekFrom::Current(pos) => self.pos = (self.pos as i64 + pos) as u64,
        }
        Ok(self.pos)
    }
}

pub fn get_size<T, A>(item: &T) -> usize
where
    for<'a> T: BinWrite<Args<'a> = A>,
    A: Default,
{
    let mut counter = Position { pos: 0 };
    item.write_le(&mut counter).unwrap();
    counter.pos as usize
}
