use colored::Colorize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use terminal_size::terminal_size;

#[derive(Debug, Clone)]
pub struct OperationRecord {
    pub name: String,
    pub count: usize,
    pub total_duration: Duration,
}

#[derive(Debug, Clone)]
pub struct SubdirStats {
    pub path: String,
    pub duration: Duration,
    pub preprocessed_count: usize,
    pub cut_count: usize,
}

#[derive(Debug)]
struct TrackerInner {
    start_time: Instant,
    operations: Vec<OperationRecord>,
    subdirs: Vec<SubdirStats>,
}

#[derive(Debug, Clone)]
pub struct RuntimeTracker {
    enabled: bool,
    inner: Arc<Mutex<TrackerInner>>,
}

impl RuntimeTracker {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            inner: Arc::new(Mutex::new(TrackerInner {
                start_time: Instant::now(),
                operations: Vec::new(),
                subdirs: Vec::new(),
            })),
        }
    }

    pub fn record_op(&self, name: &str, duration: Duration, count: usize) {
        if !self.enabled {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        if let Some(op) = inner.operations.iter_mut().find(|o| o.name == name) {
            op.count += count;
            op.total_duration += duration;
        } else {
            inner.operations.push(OperationRecord {
                name: name.to_string(),
                count,
                total_duration: duration,
            });
        }
    }

    pub fn time_op<F, R>(&self, name: &str, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        if !self.enabled {
            return f();
        }
        let start = Instant::now();
        let res = f();
        let dur = start.elapsed();
        self.record_op(name, dur, 1);
        res
    }

    pub fn record_subdir(
        &self,
        path: &str,
        duration: Duration,
        preprocessed_count: usize,
        cut_count: usize,
    ) {
        if !self.enabled || (preprocessed_count == 0 && cut_count == 0) {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        inner.subdirs.push(SubdirStats {
            path: path.to_string(),
            duration,
            preprocessed_count,
            cut_count,
        });
    }

    pub fn print_report(&self) {
        if !self.enabled {
            return;
        }
        let inner = self.inner.lock().unwrap();
        let total_wall_time = inner.start_time.elapsed();

        let term_w = terminal_size().map(|(w, _)| w.0 as usize).unwrap_or(80);
        let width = term_w.max(50);
        let border_line = "=".repeat(width);

        let title = "EXECUTION RUNTIME REPORT (--runtimes)";
        let pad = width.saturating_sub(title.len()) / 2;
        let title_line = format!("{:>pad$}{}", "", title, pad = pad);

        println!();
        println!("{}", border_line.yellow().bold());
        println!("{}", title_line.yellow().bold());
        println!("{}", border_line.yellow().bold());

        if !inner.subdirs.is_empty() {
            let sub_w = width.saturating_sub(35).max(12);
            println!(
                "\n{}",
                "--- Subdirectory Processing Summary ---".cyan().bold()
            );
            println!(
                "  {:<sub_w$} | {:>7} | {:>9} | {:>8}",
                "Subdirectory",
                "Preproc",
                "Cut Files",
                "Time",
                sub_w = sub_w
            );
            println!(
                "  {:-<sub_w$}-+-{:-<7}-+-{:-<9}-+-{:-<8}",
                "",
                "",
                "",
                "",
                sub_w = sub_w
            );
            for s in &inner.subdirs {
                let name_truncated = if s.path.len() > sub_w {
                    if sub_w > 3 {
                        format!("...{}", &s.path[s.path.len() - (sub_w - 3)..])
                    } else {
                        s.path.clone()
                    }
                } else {
                    s.path.clone()
                };
                println!(
                    "  {:<sub_w$} | {:>7} | {:>9} | {:>7.2}s",
                    name_truncated,
                    s.preprocessed_count,
                    s.cut_count,
                    s.duration.as_secs_f64(),
                    sub_w = sub_w
                );
            }
        }

        let op_w = width.saturating_sub(38).max(15);
        println!("\n{}", "--- Operation Breakdown ---".cyan().bold());
        println!(
            "  {:<op_w$} | {:>7} | {:>10} | {:>10}",
            "Operation",
            "Calls",
            "Total Time",
            "Avg / Op",
            op_w = op_w
        );
        println!(
            "  {:-<op_w$}-+-{:-<7}-+-{:-<10}-+-{:-<10}",
            "",
            "",
            "",
            "",
            op_w = op_w
        );

        let mut sum_op_dur = Duration::ZERO;
        for op in &inner.operations {
            sum_op_dur += op.total_duration;
            let avg_sec = if op.count > 0 {
                op.total_duration.as_secs_f64() / op.count as f64
            } else {
                0.0
            };
            let op_name_truncated = if op.name.len() > op_w {
                if op_w > 3 {
                    format!("...{}", &op.name[op.name.len() - (op_w - 3)..])
                } else {
                    op.name.clone()
                }
            } else {
                op.name.clone()
            };
            println!(
                "  {:<op_w$} | {:>7} | {:>9.3}s | {:>9.3}s",
                op_name_truncated,
                op.count,
                op.total_duration.as_secs_f64(),
                avg_sec,
                op_w = op_w
            );
        }

        println!(
            "  {:-<op_w$}-+-{:-<7}-+-{:-<10}-+-{:-<10}",
            "",
            "",
            "",
            "",
            op_w = op_w
        );
        let label1 = "Total Measured Operations Time";
        let label1_fmt = if label1.len() > op_w {
            &label1[..op_w]
        } else {
            label1
        };
        println!(
            "  {:<op_w$} | {:>7} | {:>9.3}s |",
            label1_fmt.bold(),
            "",
            sum_op_dur.as_secs_f64(),
            op_w = op_w
        );
        let label2 = "Total Execution Elapsed Time";
        let label2_fmt = if label2.len() > op_w {
            &label2[..op_w]
        } else {
            label2
        };
        println!(
            "  {:<op_w$} | {:>7} | {:>9.3}s |",
            label2_fmt.green().bold(),
            "",
            total_wall_time.as_secs_f64(),
            op_w = op_w
        );
        println!("{}\n", border_line.yellow().bold());
    }
}
