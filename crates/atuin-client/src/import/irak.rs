use std::path::PathBuf;
use std::str;

use async_trait::async_trait;
use atuin_common::path::PathBufExt as _;
use eyre::{Result, bail, eyre};
use jiff::{SignedDuration, Timestamp};

use super::{Importer, Loader, unix_byte_lines};
use crate::history::History;
use crate::import::read_to_end;

#[derive(Debug)]
pub struct Irak {
    bytes: Vec<u8>,
}

fn default_histpath() -> Result<PathBuf> {
    if let Ok(dir) = atuin_common::env::var("XDG_STATE_HOME")
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir).joined("rak").joined("history"));
    }

    let home_dir = dirs::home_dir().ok_or_else(|| eyre!("could not find home directory"))?;
    Ok(home_dir.joined(".local/state").joined("rak").joined("history"))
}

fn histfile_path() -> Result<PathBuf> {
    let path = default_histpath()?;
    if path.is_file() {
        Ok(path)
    } else {
        bail!(
            "Could not find irak history file {:?}. Try setting and exporting $XDG_STATE_HOME",
            path
        );
    }
}

#[async_trait]
impl Importer for Irak {
    const NAME: &'static str = "irak";

    async fn new() -> Result<Self> {
        let bytes = read_to_end(histfile_path()?)?;
        Ok(Self { bytes })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(unix_byte_lines(&self.bytes)
            .filter(|line| !line.is_empty() && str::from_utf8(line).is_ok())
            .count())
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        let commands: Vec<&str> = unix_byte_lines(&self.bytes)
            .filter_map(|line| str::from_utf8(line).ok())
            .filter(|line| !line.is_empty())
            .collect();

        // The file holds no timestamps. Space the entries one millisecond apart so
        // that the order of the file survives the import, and end near the import
        // time.
        let count = i64::try_from(commands.len()).unwrap_or(i64::MAX);
        let mut timestamp = Timestamp::now()
            .checked_sub(SignedDuration::from_millis(count))
            .unwrap_or_else(|_| Timestamp::now());

        for command in commands {
            let imported = History::import().shell("irak").timestamp(timestamp).command(command);
            h.push(imported.build().into()).await?;
            timestamp = timestamp.checked_add(SignedDuration::from_millis(1)).unwrap_or(timestamp);
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use itertools::assert_equal;
    use rstest::rstest;

    use super::Irak;
    use crate::import::Importer;
    use crate::import::tests::TestLoader;

    #[rstest]
    #[tokio::test]
    async fn parses_submissions_in_order() {
        let bytes = b"{ :a, :b }\necho if $a\n\n\necho if 1\n".to_vec();
        let mut irak = Irak { bytes };
        assert_eq!(irak.entries().await.unwrap(), 3);

        let mut loader = TestLoader::default();
        irak.load(&mut loader).await.unwrap();

        assert_equal(loader.buf.iter().map(|h| h.command.as_str()), [
            "{ :a, :b }",
            "echo if $a",
            "echo if 1",
        ]);
        assert!(loader.buf.windows(2).all(|window| window[0].timestamp < window[1].timestamp));
    }

    #[rstest]
    #[tokio::test]
    async fn skips_invalid_utf8() {
        let mut bytes = b"one\n".to_vec();
        bytes.extend_from_slice(b"\xff\xfe\n");
        bytes.extend_from_slice(b"two\n");
        let mut irak = Irak { bytes };
        assert_eq!(irak.entries().await.unwrap(), 2);

        let mut loader = TestLoader::default();
        irak.load(&mut loader).await.unwrap();

        assert_equal(loader.buf.iter().map(|h| h.command.as_str()), ["one", "two"]);
    }
}
