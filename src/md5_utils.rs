use std::io::{self, Read, Write};

// ==========================================
// Helper: Streaming MD5 Checksum
// ==========================================

/// 边写文件，边算 MD5
pub struct Md5Writer<W: Write> {
    inner: W,
    context: md5::Context,
}

impl<W: Write> Md5Writer<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            context: md5::Context::new(),
        }
    }

    // 完成计算，返回 MD5 字符串
    pub fn finish(self) -> String {
        format!("{:x}", self.context.compute())
    }
}

impl<W: Write> Write for Md5Writer<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.context.consume(buf);
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// 边读文件，边算 MD5
pub struct Md5Reader<R: Read> {
    inner: R,
    context: md5::Context,
}

impl<R: Read> Md5Reader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            context: md5::Context::new(),
        }
    }

    pub fn finish(self) -> String {
        format!("{:x}", self.context.compute())
    }
}

impl<R: Read> Read for Md5Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.context.consume(&buf[..n]);
        }
        Ok(n)
    }
}
