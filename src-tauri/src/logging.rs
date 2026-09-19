//! Log-to-file sink. Writes happen on the acquisition path's own thread, so the
//! sink holds a buffered writer and flushes in batches rather than per line.

use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFormat {
    /// `[timestamp] text`, matching the on-screen display.
    Timestamp,
    /// Raw RTT bytes, no decoration.
    Raw,
    /// `timestamp,text` with CSV field escaping.
    Csv,
}

impl SaveFormat {
    pub fn parse(s: &str) -> Self {
        match s {
            "raw" => Self::Raw,
            "csv" => Self::Csv,
            _ => Self::Timestamp,
        }
    }
}

pub struct FileSink {
    writer: BufWriter<File>,
    format: SaveFormat,
    pub path: PathBuf,
    bytes_written: u64,
}

impl FileSink {
    pub fn open(path: &Path, format: SaveFormat, append: bool) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(path)
            .with_context(|| format!("无法打开日志文件 {}", path.display()))?;
        Ok(Self {
            writer: BufWriter::with_capacity(64 * 1024, file),
            format,
            path: path.to_path_buf(),
            bytes_written: 0,
        })
    }

    /// Write one already-decoded line. `wall` is the formatted wall-clock
    /// timestamp; `text` must not contain a trailing newline.
    pub fn write_line(&mut self, wall: &str, text: &str) -> Result<()> {
        let n = match self.format {
            SaveFormat::Raw => {
                self.writer.write_all(text.as_bytes())?;
                self.writer.write_all(b"\n")?;
                text.len() as u64 + 1
            }
            SaveFormat::Timestamp => {
                let line = format!("[{}] {}\n", wall, text);
                self.writer.write_all(line.as_bytes())?;
                line.len() as u64
            }
            SaveFormat::Csv => {
                let line = format!("{},{}\r\n", csv_field(wall), csv_field(text));
                self.writer.write_all(line.as_bytes())?;
                line.len() as u64
            }
        };
        self.bytes_written += n;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

/// RFC-4180-style quoting: wrap in quotes and double any embedded quote when the
/// field contains a comma, quote, CR or LF.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
