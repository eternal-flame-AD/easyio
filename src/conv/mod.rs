use crate::read_full;
use std::io::{self, Read};

enum ReplacingReaderState {
    // the buffer has not been initialized yet
    NotInitialized,

    // the buffer is in this sequence: [4 5 6 7 0 1 2 3]
    LastReadIsMiddle,

    // the buffer is in this sequence: [0 1 2 3 4 5 6 7]
    LastReadIsStart,
}

/// ReplacingReader wraps around an underlying reader and transiently replaces given patterns in the read.
///
/// The pattern must no overlap, in such case the behavior is undefined.
/// The internal buffer is 2 * len(old_pattern), caller can wrap std::io::BufReader if more buffer is required.
///
/// A runtime panic will be thrown if old.len() == 0.
pub struct ReplacingReader<'a> {
    underlying_reader: &'a mut dyn Read,
    // buffer is separated into two parts and has a capacity of 2 * old_pattern.len()
    //
    // buffer:         X X X A | B C X X
    // next_match_ptr:       *
    // read_ptr:       *
    // next time when read_ptr is about to hit next_match_ptr, we transition to feed new to read() call
    buffer: Vec<u8>,
    old_pattern: &'a [u8],
    new_pattern: &'a [u8],
    read_ptr: usize,

    state: ReplacingReaderState,

    // this is the location of eof in the buffer, if already met
    // the last byte should be buffer[eof_position - 1]
    eof_position: Option<usize>,

    // this is the location of the next match, if present
    next_match_ptr: Option<usize>,

    // if this is Some, we are in progress of serving from new_pattern,
    // this should be set to None when serve_new_ptr == Some(new_pattern.size())
    serve_new_ptr: Option<usize>,
}

impl ReplacingReader<'_> {
    pub fn new<'a>(r: &'a mut dyn Read, old: &'a [u8], new: &'a [u8]) -> ReplacingReader<'a> {
        if old.len() ==  0 { panic!("old pattern can not be empty") };

        let buffer = vec![0; 2 * old.len()];
        ReplacingReader {
            underlying_reader: r,
            old_pattern: old,
            new_pattern: new,
            read_ptr: 0,
            buffer: buffer,
            state: ReplacingReaderState::NotInitialized,
            eof_position: None,

            next_match_ptr: None,
            serve_new_ptr: None,
        }
    }

    #[inline(always)]
    fn try_match_from(&self, start: usize) -> bool {
        let mut ptr = start;
        let mut match_len = 0usize;
        loop {
            if match_len == self.old_pattern.len() {
                return true;
            }
            if self.buffer[ptr] == self.old_pattern[match_len] {
                match_len += 1;
                ptr += 1;
                if ptr == self.buffer.len() {
                    ptr = 0;
                }
            } else {
                return false;
            }
        }
    }
}

impl Read for ReplacingReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        let buf_available = buf.len();
        // first check if we are already serving new_pattern
        if let Some(new_ptr) = self.serve_new_ptr {
            let remaining_new_pattern_len = self.new_pattern.len() - new_ptr;
            if remaining_new_pattern_len > buf_available {
                buf.copy_from_slice(&self.new_pattern[new_ptr..new_ptr + buf_available]);
                self.serve_new_ptr = Some(new_ptr + buf_available);
                return Ok(buf_available);
            } else if remaining_new_pattern_len > 0 {
                buf[..remaining_new_pattern_len].copy_from_slice(&self.new_pattern[new_ptr..]);
                self.serve_new_ptr = None;
                return Ok(remaining_new_pattern_len);
            }
        }

        // then, if this read is going to enter self.next_match_ptr?
        if let Some(next_match_ptr) = self.next_match_ptr {
            if next_match_ptr > self.read_ptr {
                let remaining_buf_available = next_match_ptr - self.read_ptr;
                if buf_available >= remaining_buf_available {
                    // we can read until start of match
                    buf[..remaining_buf_available]
                        .copy_from_slice(&self.buffer[self.read_ptr..next_match_ptr]);
                    self.serve_new_ptr = Some(0);
                    self.read_ptr = next_match_ptr + self.old_pattern.len();
                    if self.read_ptr >= self.buffer.len() {
                        self.read_ptr -= self.buffer.len();
                    }
                    self.next_match_ptr = None;
                    return Ok(remaining_buf_available);
                } else {
                    buf.copy_from_slice(&self.buffer[self.read_ptr..self.read_ptr + buf_available]);
                    self.read_ptr += buf_available;
                    return Ok(buf_available);
                }
            } else if next_match_ptr == self.read_ptr {
                self.serve_new_ptr = Some(0);
                self.read_ptr += self.old_pattern.len() ;
                if self.read_ptr >= self.buffer.len() {
                    self.read_ptr -= self.buffer.len();
                }
                self.next_match_ptr = None;
                return self.read(buf);
            } {
                let remaining_buf_available = self.buffer.len() - self.read_ptr;
                if buf_available >= remaining_buf_available {
                    buf[..remaining_buf_available].copy_from_slice(&self.buffer[self.read_ptr..]);
                    self.read_ptr = 0;
                    return Ok(remaining_buf_available);
                } else {
                    buf.copy_from_slice(&self.buffer[self.read_ptr..self.read_ptr + buf_available]);
                    self.read_ptr += buf_available;
                    return Ok(buf_available);
                }
            }
        }

        // initialize the buffer first
        match self.state {
            ReplacingReaderState::NotInitialized => {
                // first we make a full read to fill the buffer
                match read_full(&mut self.buffer, self.underlying_reader) {
                    Ok(read_len) => {
                        if read_len < self.buffer.len() {
                            // we already hit eof
                            self.eof_position = Some(read_len);
                        }
                        if read_len >= self.old_pattern.len() {
                            let possible_match_start = read_len - self.old_pattern.len();
                            for guess_start in 0..possible_match_start {
                                if self.try_match_from(guess_start) {
                                    self.next_match_ptr = Some(guess_start);
                                    break;
                                }
                            }
                        }

                        self.state = ReplacingReaderState::LastReadIsMiddle;
                        return self.read(buf);
                    }
                    Err(e) => return Err(e),
                };
            }
            _ => (),
        };

        // if we are at the end of stream and no patterns were found, nothing to do except serve the last bit of stream until end.
        if let Some(eof_position) = self.eof_position {
            // remaining buffer is from read_ptr to eof_position
            if eof_position < self.read_ptr {
                // read at most into the end of buffer
                let max_read_size = self.buffer.len() - self.read_ptr;
                if max_read_size >= self.old_pattern.len() {
                    for guess_start in self.read_ptr..self.read_ptr + 1 + max_read_size - self.old_pattern.len() {
                        if self.try_match_from(guess_start) {
                            self.next_match_ptr = Some(guess_start % self.buffer.len());
                            return self.read(buf);
                        }
                    }
                }
                if max_read_size > buf_available {
                    buf.copy_from_slice(&self.buffer[self.read_ptr..self.read_ptr + buf_available]);
                    self.read_ptr += buf_available;
                    return Ok(buf_available);
                } else {
                    buf[..max_read_size].copy_from_slice(&self.buffer[self.read_ptr..]);
                    self.read_ptr = 0;
                    return Ok(max_read_size);
                }
            } else if eof_position == self.read_ptr {
                return Ok(0);
            } else {
                let max_read_size = eof_position - self.read_ptr;
                if max_read_size >= self.old_pattern.len() {
                    for guess_start in self.read_ptr..self.read_ptr + 1 + max_read_size - self.old_pattern.len() {
                        if self.try_match_from(guess_start) {
                            self.next_match_ptr = Some(guess_start);
                            return self.read(buf);
                        }
                    }
                }
                if max_read_size > buf_available {
                    buf.copy_from_slice(&self.buffer[self.read_ptr..self.read_ptr + buf_available]);
                    self.read_ptr += buf_available;
                    return Ok(buf_available);
                } else {
                    buf[..max_read_size].copy_from_slice(&self.buffer[self.read_ptr..eof_position]);
                    self.read_ptr += max_read_size;
                    return Ok(max_read_size);
                }
            }
        }

        // here is the general case: either serve until the older half of buffer was empty or we advance buffer and do the actual pattern matching
        let wrap_pos = self.old_pattern.len();
        match self.state {
            ReplacingReaderState::LastReadIsStart => {
                if self.read_ptr >= wrap_pos {
                    let remaining_data_len = self.buffer.len() - self.read_ptr;
                    if buf_available >= remaining_data_len {
                        buf[..remaining_data_len].copy_from_slice(&self.buffer[self.read_ptr..]);
                        self.read_ptr = 0;
                        return Ok(remaining_data_len);
                    } else {
                        buf.copy_from_slice(
                            &self.buffer[self.read_ptr..self.read_ptr + buf_available],
                        );
                        self.read_ptr += buf_available;
                        return Ok(buf_available);
                    }
                }
                // next we read from the middle
                match read_full(&mut self.buffer[wrap_pos..], self.underlying_reader) {
                    Ok(size) => {
                        let mut last_possible_match_start = wrap_pos;
                        if size < self.old_pattern.len() {
                            // eof is met, set eof position
                            let eof_position = wrap_pos + size;
                            last_possible_match_start = eof_position - self.old_pattern.len()  ;
                            self.eof_position = Some(eof_position);
                        }
                        let first_possible_match_start = if self.read_ptr<1 {0} else {self.read_ptr};
                        for guess_start in first_possible_match_start..last_possible_match_start {
                            if self.try_match_from(guess_start) {
                                self.next_match_ptr = Some(guess_start);
                            }
                        }
                    }
                    Err(e) => return Err(e),

                };
                self.state = ReplacingReaderState::LastReadIsMiddle;
            }
            ReplacingReaderState::LastReadIsMiddle => {
                if self.read_ptr < wrap_pos {
                    // we still need to serve up to wrap_pos
                    let remaining_data_len = wrap_pos - self.read_ptr;
                    if buf_available >= remaining_data_len {
                        buf[..remaining_data_len]
                            .copy_from_slice(&self.buffer[self.read_ptr..wrap_pos]);
                        self.read_ptr = wrap_pos;
                        return Ok(remaining_data_len);
                    } else {
                        buf.copy_from_slice(
                            &self.buffer[self.read_ptr..self.read_ptr + buf_available],
                        );
                        self.read_ptr += buf_available;
                        return Ok(buf_available);
                    }
                }
                match read_full(&mut self.buffer[..wrap_pos], self.underlying_reader) {
                    Ok(size) => {
                        let first_possible_match_start =  if self.read_ptr > wrap_pos {self.read_ptr} else {wrap_pos };
                        let mut last_possible_match_start = self.buffer.len();
                        if size < self.old_pattern.len() {
                            let eof_position = size;
                            last_possible_match_start =
                                self.buffer.len() - self.old_pattern.len() + size;
                            self.eof_position = Some(eof_position);
                        }
                        for guess_start in first_possible_match_start..last_possible_match_start {
                            if self.try_match_from(guess_start % self.buffer.len()) {
                                self.next_match_ptr = Some(guess_start % self.buffer.len());
                            }
                        }
                    }
                    Err(e) => return Err(e),
                }
                self.state = ReplacingReaderState::LastReadIsStart;
            }
            _ => panic!("unknown state"),
        }

        return self.read(buf);
    }
}

#[cfg(test)]
mod testconv {

    mod test_replacing_reader {
        use crate::conv::ReplacingReader;
        use std::io::Read;
        use std::fmt::Write;

        fn run_string_through(input: String, old: String, new: String) -> String {
            let mut input_bytes = input.as_bytes();
            let mut reader = ReplacingReader::new(&mut input_bytes, old.as_bytes(), new.as_bytes());
            let mut ret = String::new();
            reader.read_to_string(&mut ret).unwrap();
            ret
        }


        #[test]
        fn test_varying_input_len() {
            let input_pattern = "ab";
            let old_pattern = "ab";
            let new_pattern = "cd";
            for input_len in 0..40 {
                let mut input = input_pattern.repeat(input_len/2);
                let mut expect = new_pattern.repeat(input_len/2);
                if input_len %2 == 1 {
                    input.write_char(input_pattern.chars().nth(0).unwrap()).unwrap();
                    expect.write_char(input_pattern.chars().nth(0).unwrap()).unwrap();
                }

                assert_eq!(
                    run_string_through(input, String::from(old_pattern), String::from(new_pattern)),
                    expect,
                );
            }
        }

        #[test]
        fn test_simple() {
            let input = "abcabcabcabcabc";
            let old = "ab";
            let new = "cde";
            let expect = "cdeccdeccdeccdeccdec";
            assert_eq!(
                run_string_through(String::from(input), String::from(old), String::from(new)),
                String::from(expect)
            );
        }

        #[test]
        fn test_zero_new() {
            let input = "abcabcabcabcabc";
            let old = "ab";
            let expect = "ccccc";
            assert_eq!(
                run_string_through(String::from(input), String::from(old), String::new()),
                String::from(expect)
            );
        }

        #[test]
        fn test_insert_two_places() {
            let base_str = String::from("012345678901234567890123456789");

            for n_prefix in 0..5 {
                for insert_len in 1..8usize {
                    for insert_pos_1 in 0..base_str.len() {
                        for insert_pos_2 in insert_pos_1+1..base_str.len() {
                            let mut insert_pattern = String::new();
                            for i in 0..insert_len {
                                insert_pattern.write_char(std::char::from_u32('a' as u32 + i as u32).unwrap()).unwrap();
                            }
                            let replace_to = String::from("test");

                            let mut input_str = "_".repeat(n_prefix);
                            let mut expect_str = "_".repeat(n_prefix);
                            input_str.write_str(&base_str[..insert_pos_1]).unwrap();
                            expect_str.write_str(&base_str[..insert_pos_1]).unwrap();

                            input_str.write_str(&insert_pattern).unwrap();
                            expect_str.write_str(&replace_to).unwrap();

                            input_str.write_str(&base_str[insert_pos_1..insert_pos_2]).unwrap();
                            expect_str.write_str(&base_str[insert_pos_1..insert_pos_2]).unwrap();

                            input_str.write_str(&insert_pattern).unwrap();
                            expect_str.write_str(&replace_to).unwrap();

                            input_str.write_str(&base_str[insert_pos_2..]).unwrap();
                            expect_str.write_str(&base_str[insert_pos_2..]).unwrap();

                            assert_eq!(run_string_through(input_str, insert_pattern, replace_to), expect_str);
                        }
                    }
                }
            }

        }
    }
}
