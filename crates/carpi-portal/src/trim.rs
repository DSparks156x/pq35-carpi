//! A data log's `data.csv`, trimmed: some of its columns, a time range. The
//! first column is `time_s` and rows are in time order (carpid writes it so).

use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The columns and the time range of a CSV, for the form.
#[derive(Debug, Clone, PartialEq)]
pub struct Outline {
    /// Every column after `time_s`.
    pub columns: Vec<String>,
    pub first: Option<f64>,
    pub last: Option<f64>,
}

/// The header, the first row and the last.
pub fn outline(path: &Path) -> io::Result<Outline> {
    let mut f = std::fs::File::open(path)?;
    let mut head = String::new();
    let mut r = io::BufReader::new(&mut f);
    r.read_line(&mut head)?;
    let mut first_row = String::new();
    r.read_line(&mut first_row)?;
    let columns = fields(head.trim_end_matches(['\n', '\r']))
        .into_iter()
        .skip(1)
        .map(unquote)
        .collect();
    let len = f.metadata()?.len();
    let from = len.saturating_sub(4096);
    f.seek(SeekFrom::Start(from))?;
    let mut tail = Vec::new();
    f.take(4096).read_to_end(&mut tail)?;
    let tail = String::from_utf8_lossy(&tail);
    let mut lines: Vec<&str> = tail.lines().collect();
    if from > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let last = lines.iter().rev().find_map(|l| time(l));
    Ok(Outline {
        columns,
        first: time(&first_row),
        last: last.or(time(&first_row)),
    })
}

/// What to keep. Columns are indices into a row, 0 being `time_s`, which
/// is always kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Trim {
    pub columns: Option<Vec<usize>>,
    pub from: Option<f64>,
    pub to: Option<f64>,
}

/// From the form's query: `pick=1&c=1&c=3&from=0&to=12.5`. Without `pick`
/// every column is kept; with it, at least one `c` is needed.
pub fn parse(query: &str, columns: usize) -> Result<Trim, String> {
    let mut picked = false;
    let mut cols = Vec::new();
    let (mut from, mut to) = (None, None);
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = decode(v).ok_or("Bad query.")?;
        match k {
            "pick" => picked = true,
            "c" => {
                let i: usize = v.parse().map_err(|_| "Bad column.")?;
                if i == 0 || i > columns {
                    return Err("No such column.".into());
                }
                cols.push(i);
            }
            "from" => from = seconds(&v)?,
            "to" => to = seconds(&v)?,
            _ => {}
        }
    }
    if let (Some(a), Some(b)) = (from, to)
        && a > b
    {
        return Err("From is after To.".into());
    }
    let columns = if picked {
        if cols.is_empty() {
            return Err("Pick at least one column.".into());
        }
        cols.sort_unstable();
        cols.dedup();
        Some(cols)
    } else {
        None
    };
    Ok(Trim { columns, from, to })
}

fn seconds(v: &str) -> Result<Option<f64>, String> {
    let v = v.trim();
    if v.is_empty() {
        return Ok(None);
    }
    match v.parse::<f64>() {
        Ok(n) if n.is_finite() => Ok(Some(n)),
        _ => Err(format!("{v:?} isn't a number of seconds.")),
    }
}

/// Copies the rows of `input` that `t` keeps into `out`, with the header.
/// Returns the rows written.
pub fn copy(input: impl BufRead, mut out: impl Write, t: &Trim) -> io::Result<u64> {
    let mut lines = input.lines();
    let Some(head) = lines.next().transpose()? else {
        return Ok(0);
    };
    write_fields(&mut out, &head, t)?;
    let mut n = 0;
    for l in lines {
        let l = l?;
        let Some(at) = time(&l) else { continue };
        if t.from.is_some_and(|f| at < f) {
            continue;
        }
        if t.to.is_some_and(|e| at > e) {
            break;
        }
        write_fields(&mut out, &l, t)?;
        n += 1;
    }
    out.flush()?;
    Ok(n)
}

fn write_fields(out: &mut impl Write, line: &str, t: &Trim) -> io::Result<()> {
    let line = line.trim_end_matches('\r');
    match &t.columns {
        None => writeln!(out, "{line}"),
        Some(cols) => {
            let f = fields(line);
            let mut s = String::from(f.first().copied().unwrap_or(""));
            for &c in cols {
                s.push(',');
                s.push_str(f.get(c).copied().unwrap_or(""));
            }
            writeln!(out, "{s}")
        }
    }
}

fn time(line: &str) -> Option<f64> {
    let t = line.split(',').next()?.trim();
    t.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// The fields of one line, as written (quotes kept).
fn fields(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (i, b) in line.bytes().enumerate() {
        match b {
            b'"' => quoted = !quoted,
            b',' if !quoted => {
                out.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&line[start..]);
    out
}

fn unquote(f: &str) -> String {
    match f.strip_prefix('"').and_then(|f| f.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => f.to_string(),
    }
}

/// `%xx` and `+` decoded; `None` if it isn't UTF-8.
fn decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).ok()?;
                out.push(u8::from_str_radix(h, 16).ok()?);
                i += 2;
            }
            b'%' => return None,
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "time_s,engine.rpm,\"a,b\",awd.oil_temp_c\n\
                       0.000,900,\"x,y\",\n\
                       0.100,910,,40\n\
                       0.200,,z,41\n\
                       0.300,930,,42\n";

    fn run(q: &str) -> Result<String, String> {
        let t = parse(q, 3)?;
        let mut out = Vec::new();
        copy(CSV.as_bytes(), &mut out, &t).unwrap();
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn columns_and_range() {
        assert_eq!(run("").unwrap(), CSV);
        assert_eq!(
            run("pick=1&c=3&c=1&from=0.1&to=0.2").unwrap(),
            "time_s,engine.rpm,awd.oil_temp_c\n0.100,910,40\n0.200,,41\n"
        );
        assert_eq!(
            run("pick=1&c=2&to=0.05").unwrap(),
            "time_s,\"a,b\"\n0.000,\"x,y\"\n"
        );
        assert_eq!(
            run("from=0.25&to=").unwrap(),
            "time_s,engine.rpm,\"a,b\",awd.oil_temp_c\n0.300,930,,42\n"
        );
        assert!(run("pick=1").is_err());
        assert!(run("pick=1&c=4").is_err());
        assert!(run("pick=1&c=0").is_err());
        assert!(run("from=2&to=1").is_err());
        assert!(run("from=nan").is_err());
        assert!(run("from=%zz").is_err());
        assert_eq!(decode("0.5%2B1+x").as_deref(), Some("0.5+1 x"));
        assert_eq!(decode("%4"), None);
    }

    #[test]
    fn outline_reads_header_and_ends() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("data.csv");
        std::fs::write(&p, CSV).unwrap();
        let o = outline(&p).unwrap();
        assert_eq!(o.columns, ["engine.rpm", "a,b", "awd.oil_temp_c"]);
        assert_eq!((o.first, o.last), (Some(0.0), Some(0.3)));
        // Long enough that the tail starts mid-file.
        let mut big = String::from("time_s,x\n");
        for i in 0..2000 {
            big.push_str(&format!("{}.{:03},{i}\n", i / 10, i % 10 * 100));
        }
        std::fs::write(&p, big).unwrap();
        let o = outline(&p).unwrap();
        assert_eq!((o.first, o.last), (Some(0.0), Some(199.9)));
        std::fs::write(&p, "time_s,x\n").unwrap();
        assert_eq!(outline(&p).unwrap().first, None);
    }
}
