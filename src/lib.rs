use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc};

pub mod conv;

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

/// BlackHole implements io::Write trait.
///
/// Writes to BlackHole always succeeds.
pub struct BlackHole {}
impl Write for BlackHole {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct MeteringReaderHandle<'a> {
    underlying_reader: &'a mut dyn Read,
    counter: Arc<AtomicUsize>,
}

/// MeteringReader wraps around a reader and atomically accumulates the total count of bytes written to it.
///
/// Use as_reader() to obtain a io::Reader handle to it.
pub struct MeteringReader<'a> {
    inner: MeteringReaderHandle<'a>,
    counter: Arc<AtomicUsize>,
}

impl MeteringReader<'_> {
    pub fn new(r: &mut dyn Read) -> MeteringReader {
        let counter = Arc::new(AtomicUsize::new(0));
        MeteringReader{
            inner : MeteringReaderHandle{
                underlying_reader: r,
                counter: Arc::clone(&counter),
            },
            counter: Arc::clone(&counter),
        }
    }

    pub fn as_reader(&mut self) -> &mut dyn Read {
        &mut self.inner
    }

    pub fn get_counter(&self) -> usize {
        self.counter.load(Ordering::Relaxed)
    }
}

impl Read for MeteringReaderHandle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        match self.underlying_reader.read(buf) {
            Ok(size) => {
                self.counter.fetch_add(size, Ordering::Relaxed);
                Ok(size)
            },
            Err(e) => {
                Err(e)
            }
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
        fn read(& mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
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

    mod test_metering_reader {
        use std::io;
        use crate::{MeteringReader, BlackHole};
        use std::sync::{Arc};

        #[test]
        fn test_metering_reader_update() {
            let mut input = "123456".as_bytes();
            let input_len = input.len();
            let mut meter = MeteringReader::new(&mut input);
            let mut meter_reader = meter.as_reader();
            io::copy(&mut meter_reader, &mut BlackHole{}).unwrap();
            let result = meter.get_counter();
            assert_eq!(input_len, result);
        }

        #[test]
        fn test_metering_drop_counter_when_meter_is_dropped() {
            let counter_ref;
            {
                let mut input = "123456".as_bytes();
                let input_len = input.len();
                let mut meter = MeteringReader::new(&mut input);
                counter_ref = Arc::downgrade(&meter.counter);
                let mut meter_reader = meter.as_reader();
                io::copy(&mut meter_reader, &mut BlackHole{}).unwrap();
                let result = meter.get_counter();
                assert_eq!(input_len, result);
            }
            assert_eq!(counter_ref.upgrade().is_none(), true);
        }
    }
}
