pub const USAGE: &str = "\
usage: mirage [--list] [--minimized]
  --list        print the monitors Windows currently sees, with their identities, and exit
  --minimized   start hidden in the tray (what the autostart entry uses)";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub list: bool,
    pub minimized: bool,
}

impl Args {
    pub fn parse<I: IntoIterator<Item = String>>(iter: I) -> Result<Args, String> {
        let mut args = Args::default();
        for flag in iter {
            match flag.as_str() {
                "--list" => args.list = true,
                "--minimized" => args.minimized = true,
                other => return Err(format!("unknown argument '{other}'")),
            }
        }
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Args, String> {
        Args::parse(s.split_whitespace().map(String::from))
    }

    #[test]
    fn empty_is_all_defaults() {
        assert_eq!(parse(""), Ok(Args::default()));
    }

    #[test]
    fn parses_flags() {
        assert_eq!(
            parse("--list"),
            Ok(Args {
                list: true,
                minimized: false
            })
        );
        assert_eq!(
            parse("--minimized --list"),
            Ok(Args {
                list: true,
                minimized: true
            })
        );
    }

    #[test]
    fn unknown_flag_is_an_error() {
        assert!(parse("--bogus").is_err());
        assert!(parse("stray").is_err());
    }
}
