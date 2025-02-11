//! Helpers and type definitions for extended I/O functionality
//!
//! The `io` module contains a number of types and functions to assist with common
//! I/O activities, such a slurping a file by lines, or writing a collection of `Serializable`
//! objects to a path.
//!
//! The two core parts of this module are the [`Io`] and [`DelimFile`] structs. These structs provide
//! methods for reading and writing to files that transparently handle compression based on the
//! file extension of the path given to the methods.
//!
//! ## Example
//!
//! ```rust
//! use std::{
//!     default::Default,
//!     error::Error
//! };
//! use fgoxide::io::{Io, DelimFile};
//! use serde::{Deserialize, Serialize};
//! use tempfile::TempDir;
//!
//! #[derive(Debug, Deserialize)]
//! struct SampleInfo {
//!     sample_name: String,
//!     count: usize,
//!     gene: String
//! }
//!
//! fn main() -> Result<(), Box<dyn Error>> {
//!     let tempdir = TempDir::new()?;
//!     let path = tempdir.path().join("test_file.csv.gz");
//!
//!     let io = Io::default();
//!     let lines = ["sample_name,count,gene", "sample1,100,SEPT14", "sample2,5,MIC"];
//!     io.write_lines(&path, lines.iter())?;
//!
//!     let delim = DelimFile::default();
//!     let samples: Vec<SampleInfo> = delim.read(&path, b',', false)?;
//!     assert_eq!(samples.len(), 2);
//!     assert_eq!(&samples[1].sample_name, "sample2");
//!     Ok(())
//! }
//! ```
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use crate::{FgError, Result};
use csv::{QuoteStyle, ReaderBuilder, WriterBuilder};
use flate2::bufread::MultiGzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{de::DeserializeOwned, Serialize};
use zstd::stream::{Decoder as ZstdDecoder, Encoder as ZstdEncoder};

/// The default buffer size when creating buffered readers/writers
const BUFFER_SIZE: usize = 64 * 1024;

/// The set of file extensions to treat as FASTQ, GZIPPED, or ZSTD
const FASTQ_EXTENSIONS: [&str; 2] = ["fastq", "fq"];
const GZIP_EXTENSIONS: [&str; 2] = ["gz", "bgz"];
const ZSTD_EXTENSIONS: [&str; 1] = ["zst"];

/// Unit-struct that contains associated functions for reading and writing Structs to/from
/// unstructured files.
pub struct Io {
    compression: Compression,
    buffer_size: usize,
}

/// Returns a Default implementation that will compress to gzip level 5.
impl Default for Io {
    fn default() -> Self {
        Io::new(5, BUFFER_SIZE)
    }
}

impl Io {
    /// Creates a new Io instance with the given compression level.
    pub fn new(compression: u32, buffer_size: usize) -> Io {
        Io { compression: flate2::Compression::new(compression), buffer_size }
    }

    /// Opens a file for reading. Transparently handles decoding gzip and zstd files.
    pub fn new_reader<P>(&self, p: &P) -> Result<Box<dyn BufRead + Send>>
    where
        P: AsRef<Path>,
    {
        let file = File::open(p).map_err(FgError::IoError)?;
        let buf = BufReader::with_capacity(self.buffer_size, file);

        if Self::is_gzip_path(p) {
            Ok(Box::new(BufReader::with_capacity(self.buffer_size, MultiGzDecoder::new(buf))))
        } else if Self::is_zstd_path(p) {
            Ok(Box::new(BufReader::with_capacity(
                self.buffer_size,
                ZstdDecoder::new(buf).map_err(FgError::IoError)?,
            )))
        } else {
            Ok(Box::new(buf))
        }
    }

    /// Opens a file for writing. Transparently handles encoding data in gzip and zstd formats.
    pub fn new_writer<P>(&self, p: &P) -> Result<BufWriter<Box<dyn Write + Send>>>
    where
        P: AsRef<Path>,
    {
        let file = File::create(p).map_err(FgError::IoError)?;
        let write: Box<dyn Write + Send> = if Io::is_gzip_path(p) {
            Box::new(GzEncoder::new(file, self.compression))
        } else if Io::is_zstd_path(p) {
            Box::new(ZstdEncoder::new(file, 0).map_err(FgError::IoError)?.auto_finish())
        } else {
            Box::new(file)
        };

        Ok(BufWriter::with_capacity(self.buffer_size, write))
    }

    /// Reads lines from a file into a Vec
    pub fn read_lines<P>(&self, p: &P) -> Result<Vec<String>>
    where
        P: AsRef<Path>,
    {
        let r = self.new_reader(p)?;
        let mut v = Vec::new();
        for result in r.lines() {
            v.push(result.map_err(FgError::IoError)?);
        }

        Ok(v)
    }

    /// Writes all the lines from an iterable of string-like values to a file, separated by new lines.
    pub fn write_lines<P, S>(&self, p: &P, lines: impl IntoIterator<Item = S>) -> Result<()>
    where
        P: AsRef<Path>,
        S: AsRef<str>,
    {
        let mut out = self.new_writer(p)?;
        for line in lines {
            out.write_all(line.as_ref().as_bytes()).map_err(FgError::IoError)?;
            out.write_all(&[b'\n']).map_err(FgError::IoError)?;
        }

        out.flush().map_err(FgError::IoError)
    }

    /// Returns true if the path ends with a recognized file extension
    fn is_path_with_extension<P: AsRef<Path>, const N: usize>(
        p: &P,
        extensions: [&str; N],
    ) -> bool {
        if let Some(ext) = p.as_ref().extension() {
            match ext.to_str() {
                Some(x) => extensions.contains(&x),
                None => false,
            }
        } else {
            false
        }
    }

    /// Returns true if the path ends with a recognized FASTQ file extension
    pub fn is_fastq_path<P: AsRef<Path>>(p: &P) -> bool {
        Self::is_path_with_extension(p, FASTQ_EXTENSIONS)
    }

    /// Returns true if the path ends with a recognized GZIP file extension
    pub fn is_gzip_path<P: AsRef<Path>>(p: &P) -> bool {
        Self::is_path_with_extension(p, GZIP_EXTENSIONS)
    }

    /// Returns true if the path ends with a recognized ZSTD file extension
    pub fn is_zstd_path<P: AsRef<Path>>(p: &P) -> bool {
        Self::is_path_with_extension(p, ZSTD_EXTENSIONS)
    }
}

/// Unit-struct that contains associated functions for reading and writing Structs to/from
/// delimited files.  Structs should use serde's Serialize/Deserialize derive macros in
/// order to be used with these functions.
pub struct DelimFile {
    io: Io,
}

/// Generates a default implementation that uses the default Io instance
impl Default for DelimFile {
    fn default() -> Self {
        DelimFile { io: Io::default() }
    }
}

impl DelimFile {
    /// Writes a series of one or more structs to a delimited file.  If `quote` is true then fields
    /// will be quoted as necessary, otherwise they will never be quoted.
    pub fn write<S, P>(
        &self,
        path: &P,
        recs: impl IntoIterator<Item = S>,
        delimiter: u8,
        quote: bool,
    ) -> Result<()>
    where
        S: Serialize,
        P: AsRef<Path>,
    {
        let write = self.io.new_writer(path)?;

        let mut writer = WriterBuilder::new()
            .delimiter(delimiter)
            .has_headers(true)
            .quote_style(if quote { QuoteStyle::Necessary } else { QuoteStyle::Never })
            .from_writer(write);

        for rec in recs {
            writer.serialize(rec).map_err(FgError::ConversionError)?;
        }

        writer.flush().map_err(FgError::IoError)
    }

    /// Writes structs implementing `[Serialize]` to a file with tab separators between fields.
    pub fn write_tsv<S, P>(&self, path: &P, recs: impl IntoIterator<Item = S>) -> Result<()>
    where
        S: Serialize,
        P: AsRef<Path>,
    {
        self.write(path, recs, b'\t', true)
    }

    /// Writes structs implementing `[Serialize]` to a file with comma separators between fields.
    pub fn write_csv<S, P>(&self, path: &P, recs: impl IntoIterator<Item = S>) -> Result<()>
    where
        S: Serialize,
        P: AsRef<Path>,
    {
        self.write(path, recs, b',', true)
    }

    /// Reads structs implementing `[Deserialize]` from a file with the given separators between fields.
    /// If `quote` is true then fields surrounded by quotes are parsed, otherwise quotes are not
    /// considered.
    pub fn read<D, P>(&self, path: &P, delimiter: u8, quote: bool) -> Result<Vec<D>>
    where
        D: DeserializeOwned,
        P: AsRef<Path>,
    {
        let read = self.io.new_reader(path)?;

        let mut reader = ReaderBuilder::new()
            .delimiter(delimiter)
            .has_headers(true)
            .quoting(quote)
            .from_reader(read);

        let mut results = vec![];

        for result in reader.deserialize::<D>() {
            let rec = result.map_err(FgError::ConversionError)?;
            results.push(rec);
        }

        Ok(results)
    }

    /// Reads structs implementing `[Deserialize]` from a file with tab separators between fields.
    pub fn read_tsv<D, P>(&self, path: &P) -> Result<Vec<D>>
    where
        D: DeserializeOwned,
        P: AsRef<Path>,
    {
        self.read(path, b'\t', true)
    }

    /// Reads structs implementing `[Deserialize]` from a file with tab separators between fields.
    pub fn read_csv<D, P>(&self, path: &P) -> Result<Vec<D>>
    where
        D: DeserializeOwned,
        P: AsRef<Path>,
    {
        self.read(path, b',', true)
    }
}

#[cfg(test)]
mod tests {
    use crate::io::{DelimFile, Io};
    use rstest::rstest;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    /// Record type used in testing DelimFile
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Rec {
        s: String,
        i: usize,
        b: bool,
        o: Option<f64>,
    }

    #[test]
    fn test_reading_and_writing_lines_to_file() {
        let lines = vec!["foo", "bar,splat,whee", "baz\twhoopsie"];
        let tempdir = TempDir::new().unwrap();
        let f1 = tempdir.path().join("strs.txt");
        let f2 = tempdir.path().join("Strings.txt");

        let io = Io::default();
        io.write_lines(&f1, &lines).unwrap();
        let strings: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        io.write_lines(&f2, &strings).unwrap();

        let r1 = io.read_lines(&f1).unwrap();
        let r2 = io.read_lines(&f2).unwrap();

        assert_eq!(r1, lines);
        assert_eq!(r2, lines);
    }

    #[test]
    fn test_reading_and_writing_gzip_files() {
        let lines = vec!["foo", "bar", "baz"];
        let tempdir = TempDir::new().unwrap();
        let text = tempdir.path().join("text.txt");
        let gzipped = tempdir.path().join("gzipped.txt.gz");

        let io = Io::default();
        io.write_lines(&text, &mut lines.iter()).unwrap();
        io.write_lines(&gzipped, &mut lines.iter()).unwrap();

        let r1 = io.read_lines(&text).unwrap();
        let r2 = io.read_lines(&gzipped).unwrap();

        assert_eq!(r1, lines);
        assert_eq!(r2, lines);

        // Also check that we actually wrote gzipped data to the gzip file!
        assert_ne!(text.metadata().unwrap().len(), gzipped.metadata().unwrap().len());
    }

    #[test]
    fn test_reading_and_writing_zstd_files() {
        let lines = vec!["foo", "bar", "baz"];
        let tempdir = TempDir::new().unwrap();
        let text = tempdir.path().join("text.txt");
        let zstd_compressed = tempdir.path().join("zstd_compressed.txt.zst");

        assert_eq!(Io::is_zstd_path(&text), false);
        assert_eq!(Io::is_zstd_path(&zstd_compressed), true);

        let io = Io::default();
        io.write_lines(&text, &mut lines.iter()).unwrap();
        io.write_lines(&zstd_compressed, &mut lines.iter()).unwrap();

        let r1 = io.read_lines(&text).unwrap();
        let r2 = io.read_lines(&zstd_compressed).unwrap();

        assert_eq!(r1, lines);
        assert_eq!(r2, lines);

        // Check whether the two files are different
        assert_ne!(text.metadata().unwrap().len(), zstd_compressed.metadata().unwrap().len());
    }

    #[test]
    fn test_reading_and_writing_empty_delim_file() {
        let recs: Vec<Rec> = vec![];
        let tmp = TempDir::new().unwrap();
        let csv = tmp.path().join("recs.csv");
        let tsv = tmp.path().join("recs.tsv.gz");

        let df = DelimFile::default();
        df.write_csv(&csv, &recs).unwrap();
        df.write_tsv(&tsv, &recs).unwrap();
        let from_csv: Vec<Rec> = df.read_csv(&csv).unwrap();
        let from_tsv: Vec<Rec> = df.read_tsv(&tsv).unwrap();

        assert_eq!(from_csv, recs);
        assert_eq!(from_tsv, recs);
    }

    #[test]
    fn test_reading_and_writing_delim_file() {
        let recs: Vec<Rec> = vec![
            Rec { s: "Hello".to_string(), i: 123, b: true, o: None },
            Rec { s: "A,B,C".to_string(), i: 456, b: false, o: Some(123.45) },
        ];
        let tmp = TempDir::new().unwrap();
        let csv = tmp.path().join("recs.csv");
        let tsv = tmp.path().join("recs.tsv.gz");

        let df = DelimFile::default();
        df.write_csv(&csv, &recs).unwrap();
        df.write_tsv(&tsv, &recs).unwrap();
        let from_csv: Vec<Rec> = df.read_csv(&csv).unwrap();
        let from_tsv: Vec<Rec> = df.read_tsv(&tsv).unwrap();

        assert_eq!(from_csv, recs);
        assert_eq!(from_tsv, recs);
    }

    // ############################################################################################
    // Tests is_gzip_path()
    // ############################################################################################

    #[rstest]
    #[case("test_fastq.fq.gz", true)] // .fq.gz is valid gzip
    #[case("test_fastq.fq.bgz", true)] // .fq.bgz is valid gzip
    #[case("test_fastq.fq.tar", false)] // .fq.tar is invalid gzip
    fn test_is_gzip_path(#[case] file_name: &str, #[case] expected: bool) {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join(file_name);
        let result = Io::is_gzip_path(&file_path);
        assert_eq!(result, expected);
    }

    // ############################################################################################
    // Tests is_zstd_path()
    // ############################################################################################

    #[rstest]
    #[case("test_fastq.fq", false)] // .fq is invalid zstd
    #[case("test_fastq.fq.gz", false)] // .fq.gz is invalid zstd
    #[case("test_fastq.fq.bgz", false)] // .fq.bgz is invalid zstd
    #[case("test_fastq.fq.tar", false)] // .fq.tar is invalid zstd
    #[case("test_fastq.fq.zst", true)] // .fq.zst is valid zstd
    fn test_is_zstd_path(#[case] file_name: &str, #[case] expected: bool) {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join(file_name);
        let result = Io::is_zstd_path(&file_path);
        assert_eq!(result, expected);
    }

    // ############################################################################################
    // Tests is_fastq_path()
    // ############################################################################################

    #[rstest]
    #[case("test_fastq.fq", true)] // .fq is valid fastq
    #[case("test_fastq.fastq", true)] // .fastq is valid fastq
    #[case("test_fastq.sam", false)] // .sam is invalid fastq
    fn test_is_fastq_path(#[case] file_name: &str, #[case] expected: bool) {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join(file_name);
        let result = Io::is_fastq_path(&file_path);
        assert_eq!(result, expected);
    }
}
