use metric;

use collections::hashmap::HashMap;

use time;


/// Buckets are the main storage of the statsd server. Each bucket is a simple
/// hashmap representing the key: value pairs that the statsd clients send to this
/// server.
///
/// The buckets are cleared out on every flush event.
pub struct Buckets {
    pub counters:   HashMap<~str, f64>,
    pub gauges:     HashMap<~str, f64>,
    pub histograms: HashMap<~str, ~[f64]>,
    pub timers:     HashMap<~str, ~[f64]>,

    pub server_start_time: time::Timespec,
    pub last_message: time::Timespec,
    pub bad_messages: uint,
    pub total_messages: uint
}


impl Buckets {
    pub fn new() -> Buckets {
        Buckets {
            counters: HashMap::new(),
            gauges: HashMap::new(),
            histograms: HashMap::new(),
            timers: HashMap::new(),

            server_start_time: time::get_time(),
            last_message: time::get_time(),
            bad_messages: 0,
            total_messages: 0
        }
    }

    /// Clear out current buckets
    pub fn flush(&mut self) {
        self.counters.clear();
        self.gauges.clear();
        self.histograms.clear();
        self.timers.clear();
    }

    /// Act on a line of text sent to the management server.
    ///
    /// Return a tuple of (response_str, end_conn?). If end_conn==true, close
    /// the connection.
    pub fn do_management_line(&mut self, line: &str) -> (~str, bool) {

        let mut words = line.words();

        let resp = match words.next().unwrap_or("") {
            "stats" => {
                let uptime = time::get_time().sec - self.server_start_time.sec;

                format!("uptime: {up} s\nbad messages: {bad}total messages: {total}",
                        up=uptime,
                        bad=self.bad_messages,
                        total=self.total_messages)
            },
            "clear" => {
                match words.next().unwrap_or("") {
                    "counters" => {
                        self.counters.clear();
                        ~"Counters cleared."
                    },
                    "gauges" => {
                        self.gauges.clear();
                        ~"Gauges cleared."
                    },
                    "histograms" => {
                        self.histograms.clear();
                        ~"Histograms cleared."
                    },
                    "timers" => {
                        self.timers.clear();
                        ~"Timers cleared."
                    },
                    "" => ~"ERROR: need something to clear!",
                    x => format!("ERROR: Nothing named '{}' to clear.", x)
                }
            },
            "quit" => {
                // Terminate the connection.
                return (~"END", true);
            },
            x => format!("ERROR: Unknown command: {}", x)
        };

        (resp, false)
    }

    /// Add `metric` to the proper bucket.
    pub fn add_metric(&mut self, metric: metric::Metric) {
        let key = metric.name.clone();
        let val = metric.value;

        match metric.kind {
            metric::Counter(sample_rate) => {
                self.counters
                    .insert_or_update_with(key, 0.0, |_, v| {
                        *v += val * (1.0 / sample_rate)
                    }
                );
            },
            metric::Gauge => {
                self.gauges.insert(key, val);
            },
            metric::Timer => {
                self.timers.insert_or_update_with(key, ~[], |_, v| v.push(val));
            },
            // Histograms are functionally equivalent to Timers with a
            // different name.
            metric::Histogram => {
                self.histograms.insert_or_update_with(key, ~[], |_, v| v.push(val));
            }
        }

        self.last_message = time::get_time();
        self.total_messages += 1;
    }
}
