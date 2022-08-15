#![allow(dead_code, unused_imports)]

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::os::unix::fs::{PermissionsExt, MetadataExt};

use clap::Parser;
use chrono::{DateTime, Local, Utc};
use walkdir::{Error, Result, WalkDir, DirEntry};
use md5::{Context};
use users::{get_user_by_uid, get_group_by_gid};


#[derive(Parser, Debug)]
#[clap(author, version, about="Deterministic filesystem scan summaries.")]
struct Args {
    // A flag, true if used in the command line. Note doc comment will
    // be used for the help message of the flag. The name of the
    // argument will be, by default, based on the name of the field.
    /// Activate debug mode
    #[clap(short, long)]
    debug: bool,

    // // The number of occurrences of the `v/verbose` flag
    // /// Verbose mode (-v, -vv, -vvv, etc.)
    // #[clap(short, long, parse(from_occurrences))]
    // verbose: u8,

    #[clap(short, long, default_value_t = 3)]
    maxsumsize: u64,

    #[clap(long, default_value_t = 8)]
    hashlen: u32,

    /// Files to process
    #[clap(name = "PATHS", parse(from_os_str))]
    paths: Vec<PathBuf>,
}


struct Scanner<'a> {
    args: &'a Args,
    users: HashMap::<u32, String>,
    groups: HashMap::<u32, String>,
    root: PathBuf,
    parent: PathBuf,
    dev: u64,
    count: u64,
}


impl<'a> Scanner<'a> {
    fn new(args: &'a Args) -> Self {
        Self {
            args: args,
            users: HashMap::new(),
            groups: HashMap::new(),
            root: PathBuf::new(),
            parent: PathBuf::new(),
            dev: 0,
            count: 0,
        }
    }


    fn scan(&mut self, depth: u32, dirs: Vec<PathBuf>) {
        for dir in dirs {
            if self.args.debug {
                eprintln!("{:?}", dir.metadata());
            }

            if depth == 0 {
                self.root = dir.to_path_buf();
                self.dev = dir.metadata().unwrap().dev();
                self.count = 0;

                println!("{}", "-".repeat(40));
                println!("(root) {}:", dir.to_string_lossy());
            }
            else {
                println!();
                self.parent = dir.clone();
                if let Ok(x) = dir.strip_prefix(&self.root) {
                    println!("{}/:", x.to_string_lossy());
                }
            }

            self.visit(depth, WalkDir::new(dir)
                .sort_by_file_name()
                .min_depth(1)
                .max_depth(1)
                .same_file_system(true)
            );

            if depth == 0 {
                println!("total bytes: {}", self.count);
            }
        }
    }


    fn visit(&mut self, depth: u32, walk: WalkDir) {
        let mut dirs: Vec<PathBuf> = Vec::new();

        for res in walk {
            if self.args.debug {
                eprintln!("visit {:?}", res);
            }

            if let Ok(entry) = res {
                let path = entry.path();
                let buf = path.to_path_buf();

                self.report(&buf);

                if path.is_dir() && !path.is_symlink() {
                    if path.metadata().unwrap().dev() == self.dev {
                        dirs.push(buf);
                    }
                }
            }
            else {
                println!("err {:?}", res);
            }
        }

        self.scan(depth + 1, dirs);
    }


    fn report(&mut self, path: &PathBuf) {
        let mut perms = String::new();
        let mut flen = 0;
        let mut owner = String::new();
        let mut ts = String::new();
        let mut hash = String::new();
        let mut extra = String::new();

        let fname = match path.file_name() {
            Some(name) => name.to_string_lossy(),
            None => Cow::Borrowed("?"),
        };

        let meta = if path.is_symlink() {
            std::fs::symlink_metadata(path)
        }
        else {
            path.metadata()
        };

        let otherdev;
        if let Ok(meta) = meta {
            flen = meta.len();
            perms.push_str(&unix_mode::to_string(meta.permissions().mode()));
            otherdev = meta.dev() != self.dev;
            if let Ok(mtime) = meta.modified() {
                let now: DateTime<Utc> = mtime.into();
                ts.push_str(&format!("{}", now.format("%Y-%m-%dT%H:%M")));
            }
            else {
                ts.push_str("?");
            }

            let uid = meta.uid();
            let user: &str = match self.users.get(&uid) {
                Some(name) => &name,
                None => {
                    let name = match users::get_user_by_uid(uid) {
                        Some(user) => user.name().to_string_lossy().into_owned(),
                        None => "?".into(),
                    };
                    self.users.insert(uid, name.into());
                    self.users.get(&uid).unwrap()
                }
            };

            let user: String = if user.len() > 8 {
                format!("~{:<7}", &user[user.len()-7..])
            }
            else {
                user.into()
            };


            let gid = meta.gid();
            let group: &str = match self.groups.get(&gid) {
                Some(name) => name,
                None => {
                    let name = match users::get_group_by_gid(gid) {
                        Some(grp) => grp.name().to_string_lossy().into_owned(),
                        None => "?".into(),
                    };
                    self.groups.insert(gid, name.into());
                    self.groups.get(&gid).unwrap()
                }
            };

            let group: String = if group.len() > 8 {
                format!("~{:<7}", &group[group.len()-7..])
            }
            else {
                group.into()
            };

            owner.push_str(&format!("{:8} {:8}", &user, &group));
        }
        else {
            perms.push_str("no meta");
            otherdev = false;
        }

        if path.is_symlink() {
            extra.push_str(" -> ");
            extra.push_str(&std::fs::read_link(path).unwrap().to_string_lossy());
            self.count += flen;
        }
        else if path.is_dir() {
            extra.push_str("/");
            ts.clear();
            hash.clear();
            flen = 0;
            if otherdev {
                extra.push_str(" (mountpoint)");
            }
        }
        else if path.is_file() {
            self.count += flen;

            if flen > 0 && flen < self.args.maxsumsize * 1024*1024 {
                let mut md5 = Context::new();
                if let Ok(mut file) = std::fs::File::open(&path) {
                    // println!("reading {}, len {}", path.to_string_lossy(), flen);
                    const CHUNK: usize = 1024*64;
                    let mut chunk = Vec::with_capacity(CHUNK);
                    while let Ok(n) = file.by_ref().take(CHUNK as u64).read_to_end(&mut chunk) {
                        // let mut hash = Context::new();
                        // hash.consume(&chunk[..n]);
                        // println!("read {} {}", n, hex::encode(hash.compute().0));
                        md5.consume(&chunk[..n]);
                        if n < CHUNK { break; }
                        chunk.clear();
                    }
                }
                hash.push_str(&hex::encode(md5.compute().0)[..8]);
            }
            else {
                hash.push_str(&format!("{}", "-".repeat(self.args.hashlen as usize)));
            }
        }
        else {
            // extra.push_str(" (special)");
        }

        println!("{:10} {:10} {:17} {:16} {:8} {}{}", perms, flen, owner, ts, hash, fname, extra);
    }

}


fn main() {
    let args = Args::from_args();

    let mut paths = args.paths.clone();
    if paths.len() == 0 {
        paths.push(".".into());
    }

    let mut scanner = Scanner::new(&args);
    scanner.scan(0, paths);
}


#[cfg(test)]
mod test {
    use super::*;

    // Hash of empty string
    #[test]
    fn md5() {
        let x = md5::Context::new();
        assert_eq!("d41d8cd98f00b204e9800998ecf8427e", format!("{:?}", x.compute()));
    }

}


// EOF
