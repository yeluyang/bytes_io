use std::{
    convert::{TryFrom, TryInto},
    io::Cursor,
    mem, result,
};

extern crate byteorder;
use byteorder::{ReadBytesExt, WriteBytesExt};

use crate::{
    error::{Error, Result},
    Endian,
};

#[derive(Default, Debug, PartialEq, Clone)]
pub struct Header {
    pub payload_len: u128,
    pub payload_size: u128,
    pub entry_seq: u128,
    pub frame_seq: u128,
    pub frame_total: u128,
}
pub const HEADER_SIZE: usize = mem::size_of::<Header>();

impl Header {
    fn new(length: u128, size: u128, entry_seq: u128, frame_seq: u128, total: u128) -> Self {
        assert!(length <= size);
        assert!(total > 0);
        assert!(frame_seq < total);
        Self {
            payload_len: length,
            payload_size: size,
            entry_seq,
            frame_seq,
            frame_total: total,
        }
    }
    fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();

        bytes.write_u128::<Endian>(self.payload_len)?;
        bytes.write_u128::<Endian>(self.payload_size)?;
        bytes.write_u128::<Endian>(self.entry_seq)?;
        bytes.write_u128::<Endian>(self.frame_seq)?;
        bytes.write_u128::<Endian>(self.frame_total)?;

        assert_eq!(bytes.len(), HEADER_SIZE);

        Ok(bytes)
    }
}

impl TryFrom<&[u8]> for Header {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> result::Result<Self, Self::Error> {
        assert_eq!(bytes.len(), HEADER_SIZE);

        let mut h = Self::default();

        let mut r = Cursor::new(Vec::from(bytes));
        h.payload_len = r.read_u128::<Endian>()?;
        h.payload_size = r.read_u128::<Endian>()?;
        h.entry_seq = r.read_u128::<Endian>()?;
        h.frame_seq = r.read_u128::<Endian>()?;
        h.frame_total = r.read_u128::<Endian>()?;

        Ok(h)
    }
}

impl TryFrom<Vec<u8>> for Header {
    type Error = Error;

    fn try_from(bytes: Vec<u8>) -> result::Result<Self, Self::Error> {
        Self::try_from(bytes.as_slice())
    }
}

impl TryInto<Vec<u8>> for Header {
    type Error = Error;

    fn try_into(self) -> result::Result<Vec<u8>, Self::Error> {
        self.to_bytes()
    }
}

#[derive(Default, Debug, PartialEq, Clone)]
pub struct Frame {
    pub header: Header,
    payload: Vec<u8>,
}

impl Frame {
    fn new(header: Header, mut payload: Vec<u8>) -> Self {
        trace!(
            "creating frame: header={:?}, payload.bytes={}",
            header,
            payload.len()
        );
        assert_eq!(payload.len(), header.payload_len as usize);

        if payload.len() < header.payload_size as usize {
            payload.resize_with(header.payload_size as usize, Default::default);
        };

        Self { header, payload }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = self.header.to_bytes()?;
        bytes.extend(self.payload.clone());
        assert_eq!(bytes.len(), HEADER_SIZE + self.payload.len());

        Ok(bytes)
    }

    pub fn take_payload(mut self) -> Vec<u8> {
        self.payload.truncate(self.header.payload_len as usize);
        self.payload
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..self.header.payload_len as usize]
    }

    pub fn is_first(&self) -> bool {
        self.header.frame_seq == 0
    }
}

impl TryFrom<&[u8]> for Frame {
    type Error = Error;

    fn try_from(bytes: &[u8]) -> result::Result<Self, Self::Error> {
        assert!(bytes.len() >= HEADER_SIZE);

        let header = Header::try_from(&bytes[..HEADER_SIZE])?;

        assert!(bytes.len() == HEADER_SIZE + header.payload_size as usize);
        let payload = Vec::from(&bytes[HEADER_SIZE..]);

        Ok(Self { header, payload })
    }
}

impl TryFrom<Vec<u8>> for Frame {
    type Error = Error;

    fn try_from(bytes: Vec<u8>) -> result::Result<Self, Self::Error> {
        // XXX: should move `bytes[HEADER_SIZE..]` into `frame.payload` for performance ?
        Self::try_from(bytes.as_slice())
    }
}

impl TryInto<Vec<u8>> for Frame {
    type Error = Error;

    fn try_into(self) -> result::Result<Vec<u8>, Self::Error> {
        self.to_bytes()
    }
}

pub fn frames_total(payload: usize, partial: usize) -> usize {
    assert!(partial > 0);
    let remain = if payload % partial != 0 { 1 } else { 0 };
    payload / partial as usize + remain
}

pub fn create(mut payload: Vec<u8>, entry_seq: u128, partial: usize) -> Vec<Frame> {
    assert!(partial > 0);
    let total = frames_total(payload.len(), partial);

    trace!(
        "creating {} frames: data.len={}, partial.size={}",
        total,
        payload.len(),
        partial
    );

    let mut frames: Vec<Frame> = Vec::with_capacity(total);

    for frame_seq in 0..total {
        let next = if partial < payload.len() {
            payload.split_off(partial)
        } else {
            Vec::new()
        };

        frames.push(Frame::new(
            Header::new(
                payload.len() as u128,
                partial as u128,
                entry_seq,
                frame_seq as u128,
                total as u128,
            ),
            payload,
        ));

        payload = next;
    }

    frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;

    #[test]
    fn test_frame_total() {
        init();
        struct Case {
            payloads: usize,
            partial: usize,
            frames: usize,
        }

        let cases = [Case {
            payloads: 1024,
            partial: 128,
            frames: 8,
        }];

        for c in &cases {
            let frames = frames_total(c.payloads, c.partial);
            assert_eq!(frames, c.frames)
        }
    }

    #[test]
    fn test_create() {
        init();
        struct Case {
            payload: Vec<u8>,
            partial: usize,
            entry_seq: u128,
            should_panic: bool,
            frames_num: usize,
        }
        let cases = [
            Case {
                payload: vec![1; 1024],
                partial: 128,
                entry_seq: 0,
                should_panic: false,
                frames_num: 8,
            },
            Case {
                payload: vec![1; 1024],
                partial: 0,
                entry_seq: 0,
                should_panic: true,
                frames_num: 8,
            },
            Case {
                payload: vec![2; 0],
                partial: 128,
                entry_seq: 1,
                should_panic: false,
                frames_num: 0,
            },
            Case {
                payload: vec![3; 1025],
                partial: 128,
                entry_seq: 3,
                should_panic: false,
                frames_num: 9,
            },
            Case {
                payload: vec![4; 125],
                partial: 128,
                entry_seq: 4,
                should_panic: false,
                frames_num: 1,
            },
        ];

        for c in &cases {
            match panic::catch_unwind(|| create(c.payload.clone(), c.entry_seq, c.partial)) {
                Err(_) => assert!(c.should_panic),
                Ok(frames) => {
                    assert_eq!(frames.len(), c.frames_num);

                    if c.frames_num != 0 {
                        let mut last = 0usize;
                        for (i, frame) in frames.iter().enumerate() {
                            assert!(frame.header.payload_len <= frame.header.payload_size);
                            assert_eq!(frame.header.payload_size as usize, c.partial);
                            assert_eq!(frame.header.entry_seq, c.entry_seq);
                            assert_eq!(frame.header.frame_total as usize, c.frames_num);
                            assert_eq!(frame.header.frame_seq as usize, i);
                            assert_eq!(frame.payload.len(), c.partial);
                            assert_eq!(
                                frame.payload(),
                                &c.payload[last..last + frame.header.payload_len as usize]
                            );
                            last += frame.payload.len();
                        }

                        for i in 0..frames.len() - 1 {
                            assert_eq!(frames[i].header.payload_len as usize, c.partial);

                            assert_eq!(frames[i].payload.len(), frames[i + 1].payload.len());
                            assert_eq!(
                                frames[i].header.frame_total,
                                frames[i + 1].header.frame_total
                            );
                            assert_eq!(frames[i].header.entry_seq, frames[i + 1].header.entry_seq);
                            assert_eq!(
                                frames[i].header.payload_size,
                                frames[i + 1].header.payload_size
                            );
                            assert_eq!(
                                frames[i].header.frame_seq + 1,
                                frames[i + 1].header.frame_seq
                            );
                        }

                        assert!(frames[0].is_first());

                        let frame_last = frames.last().unwrap();
                        assert_eq!(
                            frame_last.header.frame_seq + 1,
                            frame_last.header.frame_total
                        );
                        assert_eq!((frame_last.header.frame_seq + 1) as usize, frames.len());

                        let remain_bytes = c.payload.len() % c.partial;
                        if remain_bytes != 0 {
                            assert_eq!(frame_last.header.payload_len as usize, remain_bytes);
                            assert_eq!(
                                &frame_last.payload[..remain_bytes],
                                &c.payload[c.payload.len() - remain_bytes..]
                            );
                        } else {
                            assert_eq!(frame_last.header.payload_len as usize, c.partial);
                            assert_eq!(
                                frame_last.payload.as_slice(),
                                &c.payload[c.payload.len() - frame_last.payload.len()..]
                            );
                        }
                    }
                }
            };
        }
    }

    #[test]
    fn test_header() {
        init();
        struct Case {
            payload_len: u128,
            payload_size: u128,
            entry_seq: u128,
            frame_seq: u128,
            frame_total: u128,
            should_panic: bool,
        }
        let cases = [
            Case {
                payload_len: 128,
                payload_size: 128,
                entry_seq: 1,
                frame_seq: 8,
                frame_total: 16,
                should_panic: false,
            },
            Case {
                payload_len: 129, // payload_len should less than or equal to payload_size
                payload_size: 128,
                entry_seq: 1,
                frame_seq: 8,
                frame_total: 16,
                should_panic: true,
            },
            Case {
                payload_len: 128,
                payload_size: 128,
                entry_seq: 1,
                frame_seq: 16, // frame_seq should less than frame_total
                frame_total: 16,
                should_panic: true,
            },
            Case {
                payload_len: 128,
                payload_size: 128,
                entry_seq: 1,
                frame_seq: 8,
                frame_total: 0, // frame_total should greater than zero
                should_panic: true,
            },
        ];

        assert!(panic::catch_unwind(|| Header::try_from(vec![0; HEADER_SIZE / 2])).is_err());
        for c in cases.iter() {
            match panic::catch_unwind(|| {
                Header::new(
                    c.payload_len,
                    c.payload_size,
                    c.entry_seq,
                    c.frame_seq,
                    c.frame_total,
                )
            }) {
                Err(_) => assert!(c.should_panic),
                Ok(header) => {
                    let bytes = header.to_bytes().unwrap();
                    assert_eq!(bytes.len(), HEADER_SIZE);

                    let header_from = Header::try_from(bytes.as_slice()).unwrap();
                    assert_eq!(header_from, header);
                    assert_eq!(header_from.to_bytes().unwrap(), bytes);
                }
            }
        }
    }

    #[test]
    fn test_frame() {
        init();
        struct Case {
            header: Header,
            payload: Vec<u8>,
            should_panic: bool,
        }
        let cases = [
            Case {
                header: Header::new(0, 128, 0, 4, 16),
                payload: vec![0; 0],
                should_panic: false,
            },
            Case {
                header: Header::new(128, 128, 1, 8, 16),
                payload: vec![0; 128],
                should_panic: false,
            },
            Case {
                header: Header::new(64, 128, 2, 15, 16),
                payload: vec![0; 64],
                should_panic: false,
            },
            Case {
                header: Header::new(128, 128, 1, 8, 16),
                payload: vec![0; 0],
                should_panic: true,
            },
        ];

        for c in cases.iter() {
            match panic::catch_unwind(|| Frame::new(c.header.clone(), c.payload.clone())) {
                Err(_) => assert!(c.should_panic),
                Ok(frame) => {
                    assert_eq!(frame.header, c.header);
                    assert_eq!(
                        frame.payload(),
                        &frame.payload[..frame.header.payload_len as usize]
                    );

                    let bytes = frame.to_bytes().unwrap();
                    assert_eq!(bytes.len(), HEADER_SIZE + frame.payload.len());
                    assert_eq!(&bytes[..HEADER_SIZE], c.header.to_bytes().unwrap());

                    let frame_from = Frame::try_from(bytes.as_slice()).unwrap();
                    assert_eq!(frame_from, frame);
                }
            };
        }
    }
}
