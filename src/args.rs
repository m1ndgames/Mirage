use crate::geometry::Rect;

pub const USAGE: &str = "\
usage: mirage [--list] [--source N] [--target N] [--rect X,Y,W,H]
  --list        print the monitors Windows currently sees and exit
  --source N    index of the monitor to capture (default: the primary monitor)
  --target N    index of the monitor to display on (default: the smallest monitor that is not the source)
  --rect        crop rectangle relative to the source monitor's top-left corner,
                physical pixels (default: the whole source monitor)
Ctrl+C in this console quits.";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub list: bool,
    pub source: Option<usize>,
    pub target: Option<usize>,
    pub rect: Option<Rect>,
}

impl Args {
    pub fn parse<I: IntoIterator<Item = String>>(iter: I) -> Result<Args, String> {
        let mut args = Args::default();
        let mut it = iter.into_iter();
        while let Some(flag) = it.next() {
            let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
            match flag.as_str() {
                "--list" => args.list = true,
                "--source" => args.source = Some(parse_index(&value("--source")?)?),
                "--target" => args.target = Some(parse_index(&value("--target")?)?),
                "--rect" => args.rect = Some(Rect::parse(&value("--rect")?)?),
                other => return Err(format!("unknown argument '{other}'")),
            }
        }
        Ok(args)
    }
}

fn parse_index(s: &str) -> Result<usize, String> {
    s.parse::<usize>().map_err(|e| format!("invalid monitor index '{s}': {e}"))
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
    fn parses_every_flag() {
        let a = parse("--list --source 1 --target 3 --rect 1,2,3,4").unwrap();
        assert_eq!(
            a,
            Args { list: true, source: Some(1), target: Some(3), rect: Some(Rect { x: 1, y: 2, w: 3, h: 4 }) }
        );
    }

    #[test]
    fn missing_value_is_an_error() {
        assert!(parse("--source").is_err());
        assert!(parse("--rect").is_err());
    }

    #[test]
    fn unknown_flag_is_an_error() {
        assert!(parse("--bogus").is_err());
        assert!(parse("stray").is_err());
    }

    #[test]
    fn bad_values_are_errors() {
        assert!(parse("--source x").is_err());
        assert!(parse("--rect 1,2,3").is_err());
    }
}
