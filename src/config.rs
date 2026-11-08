//! `~/.config/omakeel/config.toml`: the sources `omakeel run` reads when no
//! `--source` is given. It is the flat `key = value` dialect the other apps
//! use, with `source` repeated once per source.

use crate::source::{self, Spec};
use std::{
    env,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

/// Where the config lives: `$OMAKEEL_CONFIG`, else
/// `$XDG_CONFIG_HOME/omakeel/config.toml`, else `~/.config/omakeel/config.toml`.
pub fn default_path() -> Result<PathBuf, String> {
    path_from(
        env::var_os("OMAKEEL_CONFIG"),
        env::var_os("XDG_CONFIG_HOME"),
        env::var_os("HOME"),
    )
}

fn path_from(
    own: Option<OsString>,
    xdg: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, String> {
    let absolute = |value: Option<OsString>| {
        value
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    if let Some(own) = own.filter(|v| !v.is_empty()) {
        return Ok(own.into());
    }
    let dir = match absolute(xdg) {
        Some(dir) => dir,
        None => absolute(home)
            .ok_or("HOME must be set to an absolute path to find the config")?
            .join(".config"),
    };
    Ok(dir.join("omakeel").join("config.toml"))
}

/// The sources in `text` and what could not be understood, each problem
/// naming its line. A bad line is skipped; the rest still count.
pub fn parse(text: &str) -> (Vec<Spec>, Vec<String>) {
    let mut sources = Vec::new();
    let mut problems = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        let at = n + 1;
        let Some((key, value)) = line.split_once('=') else {
            problems.push(format!("line {at}: expected key = value"));
            continue;
        };
        let (key, value) = (key.trim(), value.trim().trim_matches('"'));
        match key {
            "source" => match value.parse() {
                Ok(spec) => sources.push(spec),
                Err(e) => problems.push(format!("line {at}: {e}")),
            },
            other => problems.push(format!("line {at}: unknown setting {other}")),
        }
    }
    (sources, problems)
}

/// Reads the config at `path`. A missing file is no sources and no problem.
pub fn load(path: &Path) -> (Vec<Spec>, Vec<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (Vec::new(), Vec::new()),
        Err(e) => (Vec::new(), vec![e.to_string()]),
    }
}

/// What `omakeel run` reads: the flags if there are any, else the config.
/// Problems are lines for stderr, already prefixed with `config.toml:`.
pub fn sources(flags: Vec<Spec>, config: &Path) -> Result<(Vec<Spec>, Vec<String>), String> {
    if !flags.is_empty() {
        source::check_mix(&flags)?;
        return Ok((flags, Vec::new()));
    }
    let (sources, problems) = load(config);
    let problems: Vec<String> = problems
        .into_iter()
        .map(|problem| format!("config.toml: {problem}"))
        .collect();
    if sources.is_empty() {
        let mut message = format!(
            "no sources. Run `omahoy` to set up the boat, or pass --source SPEC \
             (or `source = SPEC` in {})",
            config.display()
        );
        for problem in &problems {
            message.push_str("\n  ");
            message.push_str(problem);
        }
        return Err(message);
    }
    source::check_mix(&sources).map_err(|e| format!("{}: {e}", config.display()))?;
    Ok((sources, problems))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> Option<OsString> {
        Some(s.into())
    }

    #[test]
    fn the_path_follows_the_override_then_xdg_then_home() {
        let home = os("/home/sailor");
        assert_eq!(
            path_from(os("/tmp/mine.toml"), os("/xdg"), home.clone()).unwrap(),
            PathBuf::from("/tmp/mine.toml")
        );
        assert_eq!(
            path_from(None, os("/xdg"), home.clone()).unwrap(),
            PathBuf::from("/xdg/omakeel/config.toml")
        );
        for unusable in [None, os(""), os("relative")] {
            assert_eq!(
                path_from(None, unusable, home.clone()).unwrap(),
                PathBuf::from("/home/sailor/.config/omakeel/config.toml")
            );
        }
        assert!(path_from(None, None, None).is_err());
    }

    #[test]
    fn reads_every_source_in_order_with_comments_and_quotes() {
        let (sources, problems) = parse(
            "# the boat\n\
             source = serial:/dev/ttyUSB0:4800\n\
             \n\
             source = \"tcp:192.168.1.20:10110\"  # the phone\n\
             source = demo\n",
        );
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(
            sources.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["serial:/dev/ttyUSB0:4800", "tcp:192.168.1.20:10110", "demo"]
        );
    }

    #[test]
    fn unknown_keys_and_bad_sources_are_reported_by_line_and_skipped() {
        let (sources, problems) = parse(
            "colour = blue\nsource = usb:/dev/x\nsource demo\nsource = serial:/dev/x:1200\nsource = demo\n",
        );
        assert_eq!(
            sources.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["demo"]
        );
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert_eq!(problems[0], "line 1: unknown setting colour");
        assert!(problems[1].starts_with("line 2: 'usb:/dev/x' is not a source"));
        assert_eq!(problems[2], "line 3: expected key = value");
        assert!(problems[3].starts_with("line 4: 1200 baud isn't supported"));
    }

    fn scratch(name: &str, text: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("omakeel-config-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn flags_replace_the_config_and_leave_it_unread() {
        let flag: Spec = "tcp:h:1".parse().unwrap();
        let path = scratch("flags", "source = demo\nbogus = 1\n");
        let (sources, problems) = sources(vec![flag.clone()], &path).unwrap();
        assert_eq!((sources, problems), (vec![flag], Vec::new()));
    }

    #[test]
    fn with_no_flags_the_config_supplies_the_sources() {
        let path = scratch("config", "source = demo\nbogus = 1\n");
        let (sources, problems) = sources(Vec::new(), &path).unwrap();
        assert_eq!(sources, [Spec::Demo]);
        assert_eq!(problems, ["config.toml: line 2: unknown setting bogus"]);
    }

    #[test]
    fn a_config_of_only_bad_lines_says_what_was_wrong_with_them() {
        let path = scratch("allbad", "source = gps\nbogus = 1\n");
        let err = sources(Vec::new(), &path).unwrap_err();
        assert!(err.starts_with("no sources."), "{err}");
        assert!(
            err.contains("config.toml: line 1: 'gps' is not a source")
                && err.contains("config.toml: line 2: unknown setting bogus"),
            "{err}"
        );
    }

    #[test]
    fn demo_beside_another_source_is_refused_from_flags_and_from_the_config() {
        let tcp: Spec = "tcp:10.0.0.1:10110".parse().unwrap();
        let missing = std::env::temp_dir().join("omakeel-config-nowhere/config.toml");
        let err = sources(vec![Spec::Demo, tcp], &missing).unwrap_err();
        assert!(
            err.contains("demo can't run beside real receivers"),
            "{err}"
        );
        let path = scratch("mixed", "source = demo\nsource = tcp:10.0.0.1:10110\n");
        let err = sources(Vec::new(), &path).unwrap_err();
        assert!(
            err.contains("demo can't run beside real receivers"),
            "{err}"
        );
        assert!(err.contains("config.toml"), "{err}");
    }

    #[test]
    fn no_sources_anywhere_says_how_to_get_some() {
        let missing = std::env::temp_dir().join("omakeel-config-nowhere/config.toml");
        for path in [missing, scratch("empty", "# nothing\n")] {
            let err = sources(Vec::new(), &path).unwrap_err();
            assert!(
                err.contains("`omahoy`") && err.contains("--source"),
                "{err}"
            );
            assert!(err.contains(&path.display().to_string()), "{err}");
        }
    }
}
