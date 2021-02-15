use std::io::{self, Read};

/// read_full reads from r until buffer is full, EOF is met, or an io:Error occured.
///
/// On ok return:
/// If return size == buffer.len() the read is successful and there may be more data available from r.
/// If return size < buffer.len(), EOF is met before buffer is filled.
pub fn read_full(buffer: &mut [u8], r: &mut dyn Read) -> Result<usize, io::Error> {
    let mut len_read: usize = 0;
    loop {
        match r.read(&mut buffer[len_read..]) {
            Ok(size) => {
                len_read += size;
                if size == 0 || buffer.len() == len_read {
                    return Ok(len_read);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate as lib;
    use std::io::{self, Read};

    pub struct SlowReader<'a> {
        underlying_reader: &'a mut dyn (Read),
    }

    impl Read for SlowReader<'_> {
        fn read<'a>(&'a mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
            // return two byte at a tim e
            if buf.len() >= 2 {
                self.underlying_reader.read(&mut buf[..2])
            } else {
                self.underlying_reader.read(&mut buf[..])
            }
        }
    }

    mod test_read_full {

        #[test]
        fn test_read_full_slow() {
            let mut underlying_data: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7];
            let mut reader = super::SlowReader {
                underlying_reader: &mut underlying_data,
            };
            let mut buf = vec![0u8; 4];
            let res = super::lib::read_full(&mut buf[..4], &mut reader);
            assert_eq!(res.unwrap(), 4usize);
            assert_eq!(buf[..4], [0, 1, 2, 3]);
            let res = super::lib::read_full(&mut buf[..3], &mut reader);
            assert_eq!(res.unwrap(), 3usize);
            assert_eq!(buf[..3], [4, 5, 6]);
            let res = super::lib::read_full(&mut buf[..3], &mut reader);
            assert_eq!(res.unwrap(), 1usize);
            assert_eq!(buf[0], 7);
        }

        #[test]
        fn test_read_full_once() {
            let mut underlying_data: &[u8] = &[0, 1, 2, 3, 4, 5, 6, 7];
            let mut buf = vec![0u8; 9];
            let res = super::lib::read_full(&mut buf[..], &mut underlying_data);
            assert_eq!(res.unwrap(), 8usize);
        }
    }
}
