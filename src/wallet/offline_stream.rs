use std::io::{self, Error, ErrorKind, Read, Write};

#[derive(Clone, Debug)]
pub struct OfflineStream {}

impl Read for OfflineStream {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(Error::new(
            ErrorKind::NotConnected,
            "Trying to read from an OfflineStream",
        ))
    }
}

impl Write for OfflineStream {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(Error::new(
            ErrorKind::NotConnected,
            "Trying to read from an OfflineStream",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(Error::new(
            ErrorKind::NotConnected,
            "Trying to read from an OfflineStream",
        ))
    }
}

// #[cfg(any(feature = "electrum", feature = "default"))]
// use electrum_client::Client;
//
// #[cfg(any(feature = "electrum", feature = "default"))]
// impl OfflineStream {
//     fn new_client() -> {
//         use std::io::bufreader;
//
//         let stream = OfflineStream{};
//         let buf_reader = BufReader::new(stream.clone());
//
//         Client {
//             stream,
//             buf_reader,
//             headers: VecDeque::new(),
//             script_notifications: BTreeMap::new(),
//
//             #[cfg(feature = "debug-calls")]
//             calls: 0,
//         }
//     }
// }
