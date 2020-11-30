use std::{
    convert::{TryFrom, TryInto},
    fs::{File, OpenOptions},
    io::{self, BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write},
    mem,
    path::{Path, PathBuf},
    result,
};

extern crate byteorder;
use byteorder::{ReadBytesExt, WriteBytesExt};

extern crate uuid;

use crate::{
    common::{self, EntryID, EntryOffset, Version},
    error::{Error, Result},
    frame::{self, Frame, Header},
    Endian,
};

#[derive(Debug, Clone, Default, PartialEq)]
struct Meta {
    version: Version,
    uuid: u128,
    header_bytes: u128,
    payload_bytes: u128,
}
const META_BYTES: usize = mem::size_of::<Meta>();

impl Meta {
    fn new(payload_bytes: u128) -> Self {
        assert!(payload_bytes > 0);
        Self {
            version: Version::new(),
            uuid: uuid::Uuid::new_v4().as_u128(),
            header_bytes: frame::HEADER_SIZE as u128,
            payload_bytes,
        }
    }

    /// length of frame in file, unit=bytes
    fn frame_bytes(&self) -> usize {
        self.header_bytes as usize + self.payload_bytes as usize
    }

    fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = self.version.to_bytes()?;
        assert_eq!(bytes.len(), common::VERSION_BYTES);

        bytes.write_u128::<Endian>(self.uuid)?;
        bytes.write_u128::<Endian>(self.header_bytes)?;
        bytes.write_u128::<Endian>(self.payload_bytes)?;

        assert_eq!(bytes.len(), META_BYTES);

        Ok(bytes)
    }
}

impl TryFrom<&[u8]> for Meta {
    type Error = Error;
    fn try_from(bytes: &[u8]) -> result::Result<Self, Self::Error> {
        assert_eq!(bytes.len(), META_BYTES);

        let version = Version::try_from(&bytes[..common::VERSION_BYTES])?;
        if !version.is_compatible() {
            return Err(Error::Incompatible(version));
        }
        let mut r = Cursor::new(Vec::from(&bytes[common::VERSION_BYTES..]));
        let uuid = r.read_u128::<Endian>()?;
        let header_bytes = r.read_u128::<Endian>()?;
        let payload_bytes = r.read_u128::<Endian>()?;

        Ok(Self {
            version,
            uuid,
            header_bytes,
            payload_bytes,
        })
    }
}

impl TryFrom<Vec<u8>> for Meta {
    type Error = Error;
    fn try_from(bytes: Vec<u8>) -> result::Result<Self, Self::Error> {
        Self::try_from(bytes.as_slice())
    }
}

impl TryInto<Vec<u8>> for Meta {
    type Error = Error;
    fn try_into(self) -> result::Result<Vec<u8>, Self::Error> {
        self.to_bytes()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub path: PathBuf,
    pub write_enable: bool,
}

impl Config {
    fn new<P: AsRef<Path>>(path: P, write_enable: bool) -> Self {
        Self {
            write_enable,
            path: PathBuf::from(path.as_ref()),
        }
    }
}

// TODO ReadOnly and WriteOnly
// TODO Concurrency-Safety
#[derive(Debug)]
pub struct BytesIO {
    pub config: Config,
    meta: Meta,
    reader: BufReader<File>,

    entry_next_seq: u128,
    frame_next_offset: usize,
    writer: Option<BufWriter<File>>,
}

impl BytesIO {
    fn new(config: Config, meta: Meta) -> Result<Self> {
        trace!("new BytesIO with: config={:?}, meta={:?}", config, meta);
        let reader = BufReader::new(OpenOptions::new().read(true).open(config.path.as_path())?);
        let writer = if config.write_enable {
            Some(BufWriter::new(
                OpenOptions::new().write(true).open(config.path.as_path())?,
            ))
        } else {
            None
        };

        Ok(Self {
            config,
            meta,
            entry_next_seq: 0,
            frame_next_offset: 0,

            reader,
            writer,
        })
    }

    /// create a new bytes_io file with write permission
    pub fn create<P: AsRef<Path>>(path: P, payload_bytes: u128) -> Result<Self> {
        assert!(payload_bytes > 0);
        trace!(
            "creating BytesIO file: on {:?}, with {} Bytes payload",
            path.as_ref(),
            payload_bytes
        );

        if path.as_ref().exists() {
            return Err(Error::FileExisted(path.as_ref().to_path_buf()));
        } else {
            File::create(path.as_ref())?;
        }

        let mut file = Self::new(Config::new(path, true), Meta::new(payload_bytes))?;

        {
            let writer = file.writer.as_mut().unwrap();
            writer.seek(SeekFrom::Start(0))?;
            writer.write_all(file.meta.to_bytes()?.as_slice())?;
            writer.flush()?;
        }

        file.reader.seek(SeekFrom::Start(META_BYTES as u64))?;

        Ok(file)
    }

    pub fn open<P: AsRef<Path>>(path: P, write_enable: bool) -> Result<Self> {
        trace!(
            "open BytesIO file: {:?}, with write_permission={}",
            path.as_ref(),
            write_enable
        );

        let mut reader = OpenOptions::new().read(true).open(path.as_ref())?;
        let mut buf = [0u8; META_BYTES];
        if let Err(err) = reader.read_exact(&mut buf) {
            match err.kind() {
                io::ErrorKind::UnexpectedEof => {
                    return Err(Error::MetaMissing(path.as_ref().to_path_buf()));
                }
                _ => {
                    return Err(Error::from(err));
                }
            };
        };
        let meta = Meta::try_from(&buf[..])?;
        debug!("read meta from BytesIO file existed: meta={:?}", meta);
        if !meta.version.is_compatible() {
            return Err(Error::Incompatible(meta.version));
        };
        drop(reader);

        let mut file = Self::new(Config::new(path.as_ref(), write_enable), meta)?;

        let frame_bytes_existed = file.reader.seek(SeekFrom::End(0))? as usize - META_BYTES;
        debug!("{} bytes of frames exists in file", frame_bytes_existed);
        if frame_bytes_existed == 0 {
            file.frame_next_offset = 0;
            file.entry_next_seq = 0;
        } else {
            assert_eq!(frame_bytes_existed % file.meta.frame_bytes(), 0);
            file.frame_next_offset = frame_bytes_existed / file.meta.frame_bytes();
            file.reader
                .seek(SeekFrom::End(-1 * file.meta.frame_bytes() as i64))?;
            file.entry_next_seq = file.read_header()?.unwrap().entry_seq + 1;
        }

        file.reader.seek(SeekFrom::Start(META_BYTES as u64))?;

        Ok(file)
    }

    pub fn read_entry(&mut self) -> Result<Option<Vec<u8>>> {
        trace!("reading entry");

        if let Some(frame_first) = self.read_frame()? {
            if frame_first.is_first() {
                let mut frame_count = 1u128;
                let mut bytes: Vec<u8> = Vec::with_capacity(
                    self.meta.payload_bytes as usize * frame_first.header.frame_total as usize,
                );
                bytes.extend(frame_first.payload());
                for _ in 0..frame_first.header.frame_total - 1 {
                    if let Some(frame) = self.read_frame()? {
                        frame_count += 1;
                        trace!(
                            "read a frame({}/{}) from an entry: header={:?}",
                            frame.header.frame_seq + 1,
                            frame_first.header.frame_total,
                            frame.header
                        );
                        assert_eq!(frame.header.entry_seq, frame_first.header.entry_seq,);
                        assert_eq!(frame.header.frame_seq + 1, frame_count,);
                        bytes.extend(frame.payload());
                    } else {
                        return Err(Error::MeetIncompleteEntry(
                            frame_first.header.frame_total,
                            frame_count,
                        ));
                    }
                }
                debug!(
                    "read entry: frames={}, sequence={}, bytes={}",
                    frame_count,
                    frame_first.header.entry_seq,
                    bytes.len(),
                );
                return Ok(Some(bytes));
            } else {
                // XXX: allow read from middle of entry?
                panic!(
                    "read from middle of entry: {} in {}",
                    frame_first.header.frame_seq + 1,
                    frame_first.header.frame_total
                );
            }
        } else {
            return Ok(None);
        };
    }

    pub fn append(&mut self, payload: &[u8]) -> Result<EntryOffset> {
        trace!("writing {} bytes into BytesIO file", payload.len(),);
        match &mut self.writer {
            None => Err(Error::WriteOnReadOnlyFile(self.config.path.clone())),
            Some(writer) => {
                let frames = frame::create(
                    payload.to_owned(),
                    self.entry_next_seq,
                    self.meta.payload_bytes as usize,
                );
                let frames_num = frames.len();

                let entry_seq = self.entry_next_seq;
                let first_frame = self.frame_next_offset;

                for frame in frames {
                    writer.write_all(frame.to_bytes()?.as_slice())?;
                    writer.flush()?;
                    self.frame_next_offset += 1;
                }
                self.entry_next_seq += 1;

                assert_eq!(self.frame_next_offset - first_frame, frames_num);
                debug!(
                    "write success, entry.seq.next: {}->{}, frames.offset.next: {}->{})",
                    entry_seq, self.entry_next_seq, first_frame, self.frame_next_offset
                );

                Ok(EntryOffset::new(self.meta.uuid, entry_seq, first_frame))
            }
        }
    }

    pub fn seek_entry(&mut self, offset: &EntryOffset) -> Result<Option<()>> {
        trace!("seek entry on offset={:?}", offset);

        if self.meta.uuid != offset.entry_id.file_id {
            Ok(None)
        } else if let Some(header) = self.seek_frame(offset.first_frame)? {
            if header.entry_seq != offset.entry_id.entry_seq {
                Err(Error::EntryMismatch(
                    offset.entry_id.entry_seq,
                    header.entry_seq,
                ))
            } else {
                Ok(Some(()))
            }
        } else {
            Ok(None)
        }
    }

    pub fn reset_reader(&mut self) -> Result<()> {
        self.seek_entry(&EntryOffset::new(self.meta.uuid, 0, 0))?;
        Ok(())
    }

    /// TODO
    pub fn first_entry(&mut self) -> Result<Option<EntryOffset>> {
        unimplemented!()
    }

    /// TODO
    pub fn last_entry(&mut self) -> Result<Option<EntryOffset>> {
        unimplemented!()
    }

    /// TODO
    pub fn find_entry(&mut self, id: EntryID) -> Result<EntryOffset> {
        unimplemented!()
    }

    /// TODO need `entry::Reserve`
    pub fn replace(&mut self, offset: EntryOffset) -> Result<()> {
        unimplemented!()
    }

    fn read_frame(&mut self) -> Result<Option<Frame>> {
        trace!("reading next frame");
        if let Some(buf) = self.read_into(self.meta.frame_bytes() as usize)? {
            let frame = Frame::try_from(buf.as_slice())?;
            debug!(
                "read next frame success: frame={{header={:?}, payload.len={}}}",
                frame.header,
                frame.payload().len(),
            );
            Ok(Some(frame))
        } else {
            debug!("next frame not found");
            Ok(None)
        }
    }

    /// TODO
    fn read_batch_frames(batch: usize) -> Result<Option<Vec<Frame>>> {
        unimplemented!()
    }

    fn read_header(&mut self) -> Result<Option<Header>> {
        trace!("reading next header of frame");
        if let Some(buf) = self.read_into(self.meta.header_bytes as usize)? {
            let header = Header::try_from(buf.as_slice())?;
            debug!("read next header of frame success: header={:?}", header,);
            Ok(Some(header))
        } else {
            debug!("next header of frame not found");
            Ok(None)
        }
    }

    fn read_into(&mut self, bytes: usize) -> Result<Option<Vec<u8>>> {
        trace!("reading {} bytes from {:?}", bytes, self.config.path);
        let mut buf: Vec<u8> = vec![0u8; bytes];
        if let Err(err) = self.reader.read_exact(buf.as_mut_slice()) {
            match err.kind() {
                io::ErrorKind::UnexpectedEof => {
                    debug!("encounter EOF of {:?}", self.config.path);
                    Ok(None)
                }
                _ => Err(Error::from(err)),
            }
        } else {
            debug!(
                "read {}/{} bytes from {:?}",
                buf.len(),
                bytes,
                self.config.path
            );
            Ok(Some(buf))
        }
    }

    fn seek_frame(&mut self, n: usize) -> Result<Option<Header>> {
        trace!("seeking frame on offset {}", n);

        let bytes = META_BYTES + self.meta.frame_bytes() * n;
        let offset = SeekFrom::Start(bytes as u64);
        if self.reader.seek(offset)? as usize == bytes {
            let result = self.read_header();
            self.reader.seek(offset)?;
            return result;
        } else {
            trace!("frame seeking not found");
            self.reader.seek(SeekFrom::End(0))?;
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tests::*;

    mod meta {
        use crate::common;

        use super::*;

        #[test]
        fn test_meta() {
            init();
            struct Case {
                // input
                payload_limit: u128,
                // output
                should_panic: bool,
            }
            let cases = &[
                Case {
                    payload_limit: 128,
                    should_panic: false,
                },
                Case {
                    payload_limit: 0,
                    should_panic: true,
                },
            ];

            for c in cases {
                match panic::catch_unwind(|| Meta::new(c.payload_limit)) {
                    Err(_) => assert!(c.should_panic),
                    Ok(meta_new) => {
                        assert_ne!(meta_new.uuid, 0);
                        assert_eq!(meta_new.version, common::CURRENT_VERSION);
                        assert_eq!(meta_new.header_bytes as usize, frame::HEADER_SIZE);
                        assert_eq!(meta_new.payload_bytes, c.payload_limit);
                        assert_eq!(
                            meta_new.frame_bytes(),
                            frame::HEADER_SIZE + c.payload_limit as usize
                        );

                        let bytes = meta_new.to_bytes().unwrap();
                        assert_eq!(bytes.len(), META_BYTES);

                        let meta_from = Meta::try_from(bytes.as_slice()).unwrap();
                        assert_eq!(meta_from, meta_new);
                        assert_eq!(meta_from.to_bytes().unwrap(), bytes);
                    }
                }
            }
        }
    }

    mod config {
        use super::*;

        struct Case {
            path: String,
            write_enable: bool,
        }

        #[test]
        fn test_new() {
            let cases = [
                Case {
                    path: "/path/to/bytesio/file".to_owned(),
                    write_enable: false,
                },
                Case {
                    path: "/path/to/bytesio/file".to_owned(),
                    write_enable: true,
                },
            ];
            for c in &cases {
                let config = Config::new(c.path.as_str(), c.write_enable);
                assert_eq!(config.path, PathBuf::from(c.path.as_str()));
                assert_eq!(config.write_enable, c.write_enable);
            }
        }
    }

    mod bytes_io {
        use std::collections::HashMap;

        use crate::frame;

        use super::*;

        fn setup<P: AsRef<Path>>(
            payload: &[&[u8]],
            path: P,
            payload_limits: u128,
        ) -> (BytesIO, HashMap<usize, EntryOffset>) {
            let mut index: HashMap<usize, EntryOffset> = HashMap::new();
            let mut file = BytesIO::create(path, payload_limits as u128).unwrap();

            for (i, data) in payload.iter().enumerate() {
                let entry_offset = file.append(data).unwrap();
                index.insert(i, entry_offset);
            }
            assert_eq!(index.len(), payload.len());
            (file, index)
        }

        #[test]
        fn test_new() {
            init();
            let case_dir = make_clean_case_dir(module_path!(), "test_new");
            let existed_path = case_dir.join("normal.bytesio");
            OpenOptions::new()
                .write(true)
                .create(true)
                .open(existed_path.as_path())
                .unwrap();

            struct Case {
                // input
                config: Config,
                meta: Meta,
                // output
                no_writer: bool,
                err: Option<Error>,
            }
            let cases = &mut [
                Case {
                    config: Config::new(existed_path.as_path(), false),
                    meta: Meta::new(128),
                    no_writer: true,
                    err: None,
                },
                Case {
                    config: Config::new(existed_path.as_path(), true),
                    meta: Meta::new(128),
                    no_writer: false,
                    err: None,
                },
                Case {
                    config: Config::new(case_dir.join("not-existed.bytesio"), false),
                    meta: Meta::new(128),
                    no_writer: true,
                    err: Some(Error::from(io::Error::from_raw_os_error(2))),
                },
            ];

            for c in cases {
                match BytesIO::new(c.config.clone(), c.meta.clone()) {
                    Err(err) => {
                        assert!(c.err.is_some());
                        assert_eq!(&err, c.err.as_ref().unwrap());
                    }
                    Ok(file) => {
                        assert_eq!(file.config, c.config);
                        assert_eq!(file.meta, c.meta);
                        assert_eq!(file.entry_next_seq, 0);
                        assert_eq!(file.frame_next_offset, 0);
                        let reader_meta = file.reader.into_inner().metadata().unwrap();
                        assert!(reader_meta.is_file());
                        match file.writer {
                            None => assert!(c.no_writer),
                            Some(_) => {
                                assert!(!c.no_writer);
                                // TODO check more about writer
                            }
                        };
                    }
                };
            }
        }

        #[test]
        fn test_create() {
            init();
            let case_dir = make_clean_case_dir(module_path!(), "test_create");
            let path_existed = case_dir.join("existed.bytesio");
            File::create(path_existed.as_path()).unwrap();

            struct Case {
                // input
                path: String,
                payload_limit: u128,
                // output
                is_panic: bool,
                err: Option<Error>,
            }
            let cases = &[
                Case {
                    path: "normal.bytesio".to_owned(),
                    payload_limit: 128,
                    is_panic: false,
                    err: None,
                },
                Case {
                    path: "should_panic_for_zero_payload_limit.bytesio".to_owned(),
                    payload_limit: 0,
                    is_panic: true,
                    err: None,
                },
                Case {
                    path: "existed.bytesio".to_owned(),
                    payload_limit: 128,
                    is_panic: false,
                    err: Some(Error::FileExisted(path_existed.clone())),
                },
                Case {
                    path: "non-existed-dir/non-existed.bytesio".to_owned(),
                    payload_limit: 128,
                    is_panic: false,
                    err: Some(Error::from(io::Error::from_raw_os_error(2))),
                },
            ];

            for c in cases {
                let path = case_dir.join(&c.path);
                match panic::catch_unwind(|| BytesIO::create(&path, c.payload_limit)) {
                    Err(_) => assert!(c.is_panic),
                    Ok(r) => match r {
                        Err(err) => assert_eq!(&err, c.err.as_ref().unwrap()),
                        Ok(mut file) => {
                            assert!(file.config.write_enable);
                            assert_eq!(file.config.path, path);

                            assert_eq!(file.meta.payload_bytes, c.payload_limit);
                            assert_eq!(
                                file.meta.frame_bytes(),
                                frame::HEADER_SIZE + c.payload_limit as usize
                            );

                            assert_eq!(file.entry_next_seq, 0);
                            assert_eq!(file.frame_next_offset, 0);

                            file.reader.seek(SeekFrom::Start(0)).unwrap();
                            let mut buf = vec![0; META_BYTES];
                            file.reader.read_exact(buf.as_mut_slice()).unwrap();
                            let meta = Meta::try_from(buf.as_slice()).unwrap();
                            assert_eq!(meta, file.meta);
                            assert_eq!(file.reader.read(buf.as_mut_slice()).unwrap(), 0);

                            assert!(BytesIO::create(&path, c.payload_limit).is_err());
                        }
                    },
                }
            }
        }

        #[test]
        fn test_open() {
            init();
            let case_dir = make_clean_case_dir(module_path!(), "test_open");

            struct Case {
                path: String,
                payload_limit: u128,
                write_enable: bool,
                dataset: &'static [&'static [u8]],
            }
            let cases = &[
                Case {
                    path: "empty.bytesio".to_owned(),
                    payload_limit: 128,
                    write_enable: false,
                    dataset: &[],
                },
                Case {
                    path: "normal.bytesio".to_owned(),
                    payload_limit: 128,
                    write_enable: false,
                    dataset: &[&[0; 64], &[1; 256]],
                },
            ];

            for c in cases {
                let path = case_dir.join(&c.path);
                setup(c.dataset, &path, c.payload_limit);

                let file = BytesIO::open(&path, c.write_enable).unwrap();

                assert_eq!(file.config.path, path);
                assert_eq!(file.config.write_enable, c.write_enable);
                assert_eq!(file.meta.payload_bytes, c.payload_limit);
                assert_eq!(
                    file.meta.frame_bytes(),
                    frame::HEADER_SIZE + c.payload_limit as usize
                );

                assert_eq!(file.entry_next_seq as usize, c.dataset.len());
                let mut frames = 0usize;
                for d in c.dataset {
                    frames += frame::frames_total(d.len(), c.payload_limit as usize);
                }
                assert_eq!(frames, file.frame_next_offset);
            }
        }

        #[test]
        fn test_io() {
            init();
            let case_dir = make_clean_case_dir(module_path!(), "test_io");

            struct Case {
                path: String,
                payload_limit: u128,
                dataset: &'static [&'static [u8]],
            }
            let cases = &[Case {
                path: "normal.bytesio".to_owned(),
                payload_limit: 128,
                dataset: &[&[0; 64], &[1; 256], &[2; 482]],
            }];

            for c in cases {
                let mut file = BytesIO::create(&case_dir.join(&c.path), c.payload_limit).unwrap();
                let mut index: Vec<EntryOffset> = vec![];

                let mut first_frame_expected = 0usize;
                let mut entry_seq_expected = 0u128;
                for d in c.dataset {
                    let frames = frame::frames_total(d.len(), c.payload_limit as usize);
                    let offset = file.append(d).unwrap();

                    assert_eq!(offset.entry_id.file_id, file.meta.uuid);

                    assert_eq!(offset.entry_id.entry_seq, entry_seq_expected);
                    assert_eq!(entry_seq_expected + 1, file.entry_next_seq);
                    entry_seq_expected = file.entry_next_seq;

                    assert_eq!(offset.first_frame, first_frame_expected);
                    assert_eq!(first_frame_expected + frames, file.frame_next_offset);
                    first_frame_expected = file.frame_next_offset;

                    let payload = file.read_entry().unwrap().unwrap();
                    assert_eq!(&payload, d);

                    index.push(offset);
                }
                assert!(file.read_entry().unwrap().is_none());

                for (i, offset) in index.iter().enumerate() {
                    file.seek_entry(offset).unwrap().unwrap();
                    let payload = file.read_entry().unwrap().unwrap();
                    assert!(i < c.dataset.len());
                    assert_eq!(&payload, &c.dataset[i]);
                }
                assert!(file.read_entry().unwrap().is_none());

                file.reset_reader().unwrap();
                for d in c.dataset {
                    let payload = file.read_entry().unwrap().unwrap();
                    assert_eq!(&payload, d);
                }
                assert!(file.read_entry().unwrap().is_none());
            }
        }
    }
}
