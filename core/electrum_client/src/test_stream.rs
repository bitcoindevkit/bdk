use std::io::{Read, Result, Write};

use std::fs::File;

pub struct TestStream {
    file: Option<File>,
    buffer: Option<Vec<u8>>,
}

impl TestStream {
    pub fn new_in(file: File) -> Self {
        TestStream {
            file: Some(file),
            buffer: None,
        }
    }

    pub fn new_out() -> Self {
        TestStream {
            file: None,
            buffer: Some(Vec::new()),
        }
    }
}

impl Read for TestStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.file.as_ref().unwrap().read(buf)
    }
}

impl Write for TestStream {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.buffer.as_mut().unwrap().write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        self.buffer.as_mut().unwrap().flush()
    }
}

impl Into<Vec<u8>> for TestStream {
    fn into(self) -> Vec<u8> {
        self.buffer.unwrap()
    }
}
