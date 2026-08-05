use colored::Colorize;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;
use terminal_size::terminal_size;
use which::which;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub postproc_enabled: bool,
    pub postproc_program: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            postproc_enabled: false,
            postproc_program: "ls".to_string(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        if let Ok(data) = fs::read_to_string(&path)
            && let Ok(cfg) = serde_json::from_str(&data)
        {
            return cfg;
        }
        let cfg = Config::default();
        let _ = cfg.save();
        cfg
    }

    pub fn save(&self) -> io::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::File::create(&path)?;
        writeln!(file, "{{")?;
        writeln!(
            file,
            "  \"$schema\": \"https://json.schemastore.org/config\","
        )?;
        writeln!(
            file,
            "  \"_comment\": \"Configuration for podcasts_remove_ads.\","
        )?;
        writeln!(file, "  \"_comment_postproc_enabled\": \"If true, run a post-processing program after each successful cut.\",")?;
        writeln!(file, "  \"postproc_enabled\": {},", self.postproc_enabled)?;
        writeln!(file, "  \"_comment_postproc_program\": \"Name or path of the post-processing program to run (receives the cut file path as argument).\",")?;
        writeln!(
            file,
            "  \"postproc_program\": \"{}\"",
            self.postproc_program
        )?;
        writeln!(file, "}}")?;
        file.flush()?;
        Ok(())
    }

    pub fn run_postproc(&self, file_path: &std::path::Path) {
        if !self.postproc_enabled {
            return;
        }
        let program = self.postproc_program.trim();
        if program.is_empty() {
            return;
        }
        let program_path = match which(program) {
            Ok(p) => p,
            Err(_) => {
                eprintln!("Error: post-processing program not found: {}", program);
                return;
            }
        };
        let width = terminal_size().map(|(w, _)| w.0 as usize).unwrap_or(72);
        let fname = file_path.display().to_string();
        let title = format!("─── Post-processor: {} ", fname);
        let pad = width.saturating_sub(title.len() + 1);
        let bar = "│".cyan();
        let dash = "─".cyan();
        println!();
        println!("{}{}{}", "╭".cyan(), title.cyan().bold(), dash.repeat(pad));
        match Command::new(program_path)
            .arg(file_path)
            .output()
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                for line in stdout.lines() {
                    println!("{} {}", bar, line);
                }
                for line in stderr.lines() {
                    eprintln!("{} {}", bar, line);
                }
            }
            Err(e) => {
                eprintln!("{} {}", bar, e);
            }
        }
        println!("{}{}", "╰".cyan(), dash.repeat(width - 1));
        println!();
    }

    pub fn show(&self) {
        let enabled = if self.postproc_enabled {
            "enabled".green().bold()
        } else {
            "disabled".red().bold()
        };
        println!();
        println!("{}", "Configuration".yellow().bold().underline());
        println!();
        println!("{}  {}", "Post-processor:".cyan().bold(), enabled);
        println!(
            "  {}  {}",
            "Program:".cyan().bold(),
            self.postproc_program
        );
        println!();
        println!(
            "  {}",
            "Usage: podcasts_remove_ads config [options]".bright_black()
        );
        println!(
            "  {}",
            "  --postproc on|off    Enable or disable the post-processor".bright_black()
        );
        println!(
            "  {}",
            "  --postproc-set <cmd> Set the post-processing program".bright_black()
        );
        println!(
            "  {}",
            "  --show               Display this configuration".bright_black()
        );
        println!();
    }
}

fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/podcasts_remove_ads/config.json")
}
