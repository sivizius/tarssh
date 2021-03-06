macro_rules! metric_bucket {
    ($Name:ident ($Bucket:expr): $($Attributes:expr),* $(,)?)
    => {concat!(stringify!($Name), "{{", $($Attributes),*, "}} {", stringify!($Bucket), "}\n",)};

}

macro_rules! metric_type {
    (counter)   => {"counter"};
    (gauge)     => {"gauge"};
    (histogram) => {"histogram"};
    (summary)   => {"summary"};
    (untyped)   => {"untyped"};
}

macro_rules! metric_header {
  (
      $Name:ident:
      $Type:ident,
      $Description:expr$(,)?
  ) => {
      concat!(
          "# HELP ", stringify!($Name), " ", $Description, "\n",
          "# TYPE ", stringify!($Name), " ", metric_type!($Type), "\n"
      )
  };
}

macro_rules! metric {
    (
        $Name:ident:
        $Type:ident,
        $Description:expr$(,)?
    ) => {
        concat!(
            metric_header!($Name: $Type, $Description),
            stringify!($Name), " {", stringify!($Name), "}\n\n",
        );
    };
}

use std::{
    borrow::Cow,
    sync::{atomic::{AtomicUsize, Ordering}, Mutex},
    time::Instant,
};

pub(crate) struct Client {
    start:            Instant,
    sent_chunks:      u64,
    sent_eastereggs:  u64,
    sent_banners:     u64,
}

pub(crate) struct ClientMetrics {
    maximum_connection_time:  u64,
    minimum_connection_time:  u64,
    connection_time_till:     [usize; 32],
    connection_time:          u64,
    sent_chunks_sum:          u64,
    sent_eastereggs_sum:      u64,
    sent_banners_sum:         u64,
}

impl ClientMetrics {
    pub(crate) fn new() -> Self {
        Self {
            maximum_connection_time:  0,
            minimum_connection_time:  u64::MAX,
            connection_time_till:     [0usize; 32],
            connection_time:          0,
            sent_chunks_sum:          0,
            sent_eastereggs_sum:      0,
            sent_banners_sum:         0,
        }
    }
}

pub(crate) struct Metrics {
    startup:            Instant,
    clients:            Mutex<Vec<Option<Client>>>,
    former_metrics:     Mutex<ClientMetrics>,
    connections_count:  AtomicUsize,
    connections_total:  AtomicUsize,
}

impl Metrics {
    pub(crate) fn new(
        startup: Instant,
    ) -> Self {
        Self {
            startup,
            clients:            Mutex::new(Vec::new()),
            former_metrics:     Mutex::new(ClientMetrics::new()),
            connections_count:  AtomicUsize::new(0),
            connections_total:  AtomicUsize::new(0),
        }
    }

    pub(crate) fn connections(&self) -> usize {
        self.connections_count.load(Ordering::Relaxed)
    }

    pub(crate) fn connect(
        &self,
        max_clients: usize,
        start: Instant,
    ) -> Result<(usize, Token), usize> {
        self.connections_total.fetch_add(1, Ordering::Relaxed);
        let connected = self.connections_count.fetch_add(1, Ordering::Relaxed) + 1;
        if connected > max_clients {
            self.connections_count.fetch_sub(1, Ordering::Relaxed);
            Err(connected)
        } else {
            let client = Client {
                start,
                sent_chunks:      0,
                sent_eastereggs:  0,
                sent_banners:     0,
            };
            let mut guard = match self.clients.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            Ok((
                connected,
                Token {
                    uid: if let Some(index) = guard
                        .iter()
                        .enumerate()
                        .find_map(|(index, value)|
                            if value.is_none() {
                                Some(index)
                            }
                            else {
                                None
                            }
                        ) {
                        guard [ index ] = Some(client);
                        index
                    } else {
                        guard.push(Some(client));
                        guard.len() - 1
                    }
                },
            ))
        }
    }

    pub(crate) fn disconnect(
        &self,
        token: Token,
    ) -> Result<(usize, u64), Cow<'static, str>> {
      let mut guard = match self.clients.lock() {
          Ok(guard) => guard,
          Err(poisoned) => poisoned.into_inner(),
      };
      let mut metrics_guard = match self.former_metrics.lock() {
          Ok(guard) => guard,
          Err(poisoned) => poisoned.into_inner(),
      };
      if guard.len() > token.uid {
          if let Some(ref client) = guard[token.uid] {
              let connected = self.connections_count.fetch_sub(1, Ordering::Relaxed);
              let connection_time = client.start.elapsed().as_secs();
              metrics_guard.maximum_connection_time = metrics_guard.maximum_connection_time.max(connection_time);
              metrics_guard.minimum_connection_time = metrics_guard.minimum_connection_time.min(connection_time);
              let bucket = 63-connection_time.leading_zeros() as usize;
              metrics_guard.connection_time_till[bucket] += 1;
              metrics_guard.connection_time     += connection_time;
              metrics_guard.sent_chunks_sum     += client.sent_chunks;
              metrics_guard.sent_eastereggs_sum += client.sent_eastereggs;
              metrics_guard.sent_banners_sum    += client.sent_banners;
              guard[token.uid] = None;
              Ok((connected-1, connection_time))
          } else {
              Err(Cow::Borrowed("Already Disconnected"))
          }
      } else {
          Err(Cow::Borrowed("Invalid Token"))
      }
    }

    pub(crate) fn export(&self) -> String {
        let client_guard = match self.clients.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let client_metrics = client_guard
            .iter()
            .fold(
                ClientMetrics::new(),
                |mut metrics, client| {
                    if let Some(client) = client {
                        let connection_time = client.start.elapsed().as_secs();
                        metrics.maximum_connection_time = metrics.maximum_connection_time.max(connection_time);
                        metrics.minimum_connection_time = metrics.minimum_connection_time.min(connection_time);
                        let bucket = 63-connection_time.leading_zeros() as usize;
                        metrics.connection_time_till[bucket] += 1;
                        metrics.connection_time     += connection_time;
                        metrics.sent_chunks_sum     += client.sent_chunks;
                        metrics.sent_eastereggs_sum += client.sent_eastereggs;
                        metrics.sent_banners_sum    += client.sent_banners;
                    }
                    metrics
                }
            );
        let former_metrics = match self.former_metrics.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        format!(
            concat!(
                metric!       (uptime_seconds:                          gauge,      "Number of seconds since startup."                              ),
                metric!       (connections_count:                       counter,    "Number of current connections."                                ),
                metric!       (connections_total:                       counter,    "Total number of connections."                                  ),
                metric!       (client_maximum_connection_time_seconds:  counter,    "Length in seconds of longest connection by current clients."   ),
                metric!       (client_minimum_connection_time_seconds:  counter,    "Length in seconds of shortest connection by current clients."  ),
                metric!       (client_sent_chunks_sum:                  counter,    "Sum of sent chunks by current clients."                        ),
                metric!       (client_sent_eastereggs_sum:              counter,    "Sum of sent sent_eastereggs by current clients."               ),
                metric!       (client_sent_banners_sum:                 counter,    "Sum of sent banners by current clients."                       ),
                metric!       (client_connection_time_seconds_sum:      counter,    "Sum of connection time of current clients."                    ),
                metric_header!(client_connection_time_seconds_bucket:   histogram,  "A histogram of the connection time of current clients."        ),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket00):  "le=0s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket01):  "le=1s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket02):  "le=3s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket03):  "le=7s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket04):  "le=15s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket05):  "le=31s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket06):  "le=63s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket07):  "le=127s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket08):  "le=255s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket09):  "le=511s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket0a):  "le=1023s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket0b):  "le=2047s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket0c):  "le=4095s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket0d):  "le=8191s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket0e):  "le=16383s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket0f):  "le=32767s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket10):  "le=65535s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket11):  "le=131071s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket12):  "le=262143s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket13):  "le=524287s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket14):  "le=1048575s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket15):  "le=2097151s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket16):  "le=4194303s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket17):  "le=8388607s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket18):  "le=16777215s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket19):  "le=33554431s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket1a):  "le=67108863s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket1b):  "le=134217727s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket1c):  "le=268435455s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket1d):  "le=536870911s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket1e):  "le=1073741823s",),
                metric_bucket!(client_connection_time_seconds_bucket (client_connection_time_bucket1f):  "le=2147483647s",),
                "\n",
                metric!       (former_maximum_connection_time_seconds:  counter,    "Length in seconds of longest connection by former clients."  ),
                metric!       (former_minimum_connection_time_seconds:  counter,    "Length in seconds of shortest connection by former clients." ),
                metric!       (former_sent_chunks_sum:                  counter,    "Sum of sent chunks by former clients."                       ),
                metric!       (former_sent_eastereggs_sum:              counter,    "Sum of sent sent_eastereggs by former clients."              ),
                metric!       (former_sent_banners_sum:                 counter,    "Sum of sent banners by former clients."                      ),
                metric!       (former_connection_time_seconds_sum:      counter,    "Sum of connection time of former clients."                    ),
                metric_header!(former_connection_time_seconds_bucket:   histogram,  "A histogram of the connection time of former clients."       ),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket00):  "le=0s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket01):  "le=1s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket02):  "le=3s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket03):  "le=7s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket04):  "le=15s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket05):  "le=31s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket06):  "le=63s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket07):  "le=127s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket08):  "le=255s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket09):  "le=511s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket0a):  "le=1023s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket0b):  "le=2047s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket0c):  "le=4095s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket0d):  "le=8191s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket0e):  "le=16383s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket0f):  "le=32767s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket10):  "le=65535s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket11):  "le=131071s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket12):  "le=262143s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket13):  "le=524287s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket14):  "le=1048575s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket15):  "le=2097151s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket16):  "le=4194303s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket17):  "le=8388607s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket18):  "le=16777215s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket19):  "le=33554431s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket1a):  "le=67108863s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket1b):  "le=134217727s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket1c):  "le=268435455s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket1d):  "le=536870911s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket1e):  "le=1073741823s",),
                metric_bucket!(former_connection_time_seconds_bucket (former_connection_time_bucket1f):  "le=2147483647s",),
                "\n",
                metric!       (total_maximum_connection_time_seconds:  counter,    "Length in seconds of longest connection overall."   ),
                metric!       (total_minimum_connection_time_seconds:  counter,    "Length in seconds of shortest connection overall."  ),
                metric!       (total_sent_chunks_sum:                  counter,    "Sum of sent chunks overall."                        ),
                metric!       (total_sent_eastereggs_sum:              counter,    "Sum of sent sent_eastereggs overall."               ),
                metric!       (total_sent_banners_sum:                 counter,    "Sum of sent banners overall."                       ),
                metric!       (total_connection_time_seconds_sum:      counter,    "Sum of connection time overall."                    ),
                metric_header!(total_connection_time_seconds_bucket:   histogram,  "A histogram of the connection time overall."        ),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket00):  "le=0s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket01):  "le=1s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket02):  "le=3s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket03):  "le=7s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket04):  "le=15s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket05):  "le=31s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket06):  "le=63s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket07):  "le=127s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket08):  "le=255s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket09):  "le=511s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket0a):  "le=1023s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket0b):  "le=2047s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket0c):  "le=4095s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket0d):  "le=8191s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket0e):  "le=16383s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket0f):  "le=32767s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket10):  "le=65535s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket11):  "le=131071s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket12):  "le=262143s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket13):  "le=524287s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket14):  "le=1048575s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket15):  "le=2097151s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket16):  "le=4194303s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket17):  "le=8388607s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket18):  "le=16777215s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket19):  "le=33554431s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket1a):  "le=67108863s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket1b):  "le=134217727s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket1c):  "le=268435455s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket1d):  "le=536870911s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket1e):  "le=1073741823s",),
                metric_bucket!(total_connection_time_seconds_bucket (total_connection_time_bucket1f):  "le=2147483647s",),
            ),
            uptime_seconds                          = self.startup.elapsed().as_secs(),
            connections_count                       = self.connections_count.load(Ordering::Relaxed),
            connections_total                       = self.connections_total.load(Ordering::Relaxed),
            client_maximum_connection_time_seconds  = client_metrics.maximum_connection_time,
            client_minimum_connection_time_seconds  = client_metrics.minimum_connection_time,
            client_sent_chunks_sum                  = client_metrics.sent_chunks_sum,
            client_sent_eastereggs_sum              = client_metrics.sent_eastereggs_sum,
            client_sent_banners_sum                 = client_metrics.sent_banners_sum,
            client_connection_time_seconds_sum      = client_metrics.connection_time,
            client_connection_time_bucket00         = client_metrics.connection_time_till[0x00],
            client_connection_time_bucket01         = client_metrics.connection_time_till[0x01],
            client_connection_time_bucket02         = client_metrics.connection_time_till[0x02],
            client_connection_time_bucket03         = client_metrics.connection_time_till[0x03],
            client_connection_time_bucket04         = client_metrics.connection_time_till[0x04],
            client_connection_time_bucket05         = client_metrics.connection_time_till[0x05],
            client_connection_time_bucket06         = client_metrics.connection_time_till[0x06],
            client_connection_time_bucket07         = client_metrics.connection_time_till[0x07],
            client_connection_time_bucket08         = client_metrics.connection_time_till[0x08],
            client_connection_time_bucket09         = client_metrics.connection_time_till[0x09],
            client_connection_time_bucket0a         = client_metrics.connection_time_till[0x0a],
            client_connection_time_bucket0b         = client_metrics.connection_time_till[0x0b],
            client_connection_time_bucket0c         = client_metrics.connection_time_till[0x0c],
            client_connection_time_bucket0d         = client_metrics.connection_time_till[0x0d],
            client_connection_time_bucket0e         = client_metrics.connection_time_till[0x0e],
            client_connection_time_bucket0f         = client_metrics.connection_time_till[0x0f],
            client_connection_time_bucket10         = client_metrics.connection_time_till[0x10],
            client_connection_time_bucket11         = client_metrics.connection_time_till[0x11],
            client_connection_time_bucket12         = client_metrics.connection_time_till[0x12],
            client_connection_time_bucket13         = client_metrics.connection_time_till[0x13],
            client_connection_time_bucket14         = client_metrics.connection_time_till[0x14],
            client_connection_time_bucket15         = client_metrics.connection_time_till[0x15],
            client_connection_time_bucket16         = client_metrics.connection_time_till[0x16],
            client_connection_time_bucket17         = client_metrics.connection_time_till[0x17],
            client_connection_time_bucket18         = client_metrics.connection_time_till[0x18],
            client_connection_time_bucket19         = client_metrics.connection_time_till[0x19],
            client_connection_time_bucket1a         = client_metrics.connection_time_till[0x1a],
            client_connection_time_bucket1b         = client_metrics.connection_time_till[0x1b],
            client_connection_time_bucket1c         = client_metrics.connection_time_till[0x1c],
            client_connection_time_bucket1d         = client_metrics.connection_time_till[0x1d],
            client_connection_time_bucket1e         = client_metrics.connection_time_till[0x1e],
            client_connection_time_bucket1f         = client_metrics.connection_time_till[0x1f],
            former_maximum_connection_time_seconds  = former_metrics.maximum_connection_time,
            former_minimum_connection_time_seconds  = former_metrics.minimum_connection_time,
            former_sent_chunks_sum                  = former_metrics.sent_chunks_sum,
            former_sent_eastereggs_sum              = former_metrics.sent_eastereggs_sum,
            former_sent_banners_sum                 = former_metrics.sent_banners_sum,
            former_connection_time_seconds_sum      = former_metrics.connection_time,
            former_connection_time_bucket00         = former_metrics.connection_time_till[0x00],
            former_connection_time_bucket01         = former_metrics.connection_time_till[0x01],
            former_connection_time_bucket02         = former_metrics.connection_time_till[0x02],
            former_connection_time_bucket03         = former_metrics.connection_time_till[0x03],
            former_connection_time_bucket04         = former_metrics.connection_time_till[0x04],
            former_connection_time_bucket05         = former_metrics.connection_time_till[0x05],
            former_connection_time_bucket06         = former_metrics.connection_time_till[0x06],
            former_connection_time_bucket07         = former_metrics.connection_time_till[0x07],
            former_connection_time_bucket08         = former_metrics.connection_time_till[0x08],
            former_connection_time_bucket09         = former_metrics.connection_time_till[0x09],
            former_connection_time_bucket0a         = former_metrics.connection_time_till[0x0a],
            former_connection_time_bucket0b         = former_metrics.connection_time_till[0x0b],
            former_connection_time_bucket0c         = former_metrics.connection_time_till[0x0c],
            former_connection_time_bucket0d         = former_metrics.connection_time_till[0x0d],
            former_connection_time_bucket0e         = former_metrics.connection_time_till[0x0e],
            former_connection_time_bucket0f         = former_metrics.connection_time_till[0x0f],
            former_connection_time_bucket10         = former_metrics.connection_time_till[0x10],
            former_connection_time_bucket11         = former_metrics.connection_time_till[0x11],
            former_connection_time_bucket12         = former_metrics.connection_time_till[0x12],
            former_connection_time_bucket13         = former_metrics.connection_time_till[0x13],
            former_connection_time_bucket14         = former_metrics.connection_time_till[0x14],
            former_connection_time_bucket15         = former_metrics.connection_time_till[0x15],
            former_connection_time_bucket16         = former_metrics.connection_time_till[0x16],
            former_connection_time_bucket17         = former_metrics.connection_time_till[0x17],
            former_connection_time_bucket18         = former_metrics.connection_time_till[0x18],
            former_connection_time_bucket19         = former_metrics.connection_time_till[0x19],
            former_connection_time_bucket1a         = former_metrics.connection_time_till[0x1a],
            former_connection_time_bucket1b         = former_metrics.connection_time_till[0x1b],
            former_connection_time_bucket1c         = former_metrics.connection_time_till[0x1c],
            former_connection_time_bucket1d         = former_metrics.connection_time_till[0x1d],
            former_connection_time_bucket1e         = former_metrics.connection_time_till[0x1e],
            former_connection_time_bucket1f         = former_metrics.connection_time_till[0x1f],
            total_maximum_connection_time_seconds   = client_metrics.maximum_connection_time.max(former_metrics.maximum_connection_time),
            total_minimum_connection_time_seconds   = client_metrics.minimum_connection_time.min(former_metrics.maximum_connection_time),
            total_sent_chunks_sum                   = client_metrics.sent_chunks_sum      + former_metrics.sent_chunks_sum,
            total_sent_eastereggs_sum               = client_metrics.sent_eastereggs_sum  + former_metrics.sent_eastereggs_sum,
            total_sent_banners_sum                  = client_metrics.sent_banners_sum     + former_metrics.sent_banners_sum,
            total_connection_time_seconds_sum       = client_metrics.connection_time      + former_metrics.connection_time,
            total_connection_time_bucket00          = client_metrics.connection_time_till[0x00] + former_metrics.connection_time_till[0x00],
            total_connection_time_bucket01          = client_metrics.connection_time_till[0x01] + former_metrics.connection_time_till[0x01],
            total_connection_time_bucket02          = client_metrics.connection_time_till[0x02] + former_metrics.connection_time_till[0x02],
            total_connection_time_bucket03          = client_metrics.connection_time_till[0x03] + former_metrics.connection_time_till[0x03],
            total_connection_time_bucket04          = client_metrics.connection_time_till[0x04] + former_metrics.connection_time_till[0x04],
            total_connection_time_bucket05          = client_metrics.connection_time_till[0x05] + former_metrics.connection_time_till[0x05],
            total_connection_time_bucket06          = client_metrics.connection_time_till[0x06] + former_metrics.connection_time_till[0x06],
            total_connection_time_bucket07          = client_metrics.connection_time_till[0x07] + former_metrics.connection_time_till[0x07],
            total_connection_time_bucket08          = client_metrics.connection_time_till[0x08] + former_metrics.connection_time_till[0x08],
            total_connection_time_bucket09          = client_metrics.connection_time_till[0x09] + former_metrics.connection_time_till[0x09],
            total_connection_time_bucket0a          = client_metrics.connection_time_till[0x0a] + former_metrics.connection_time_till[0x0a],
            total_connection_time_bucket0b          = client_metrics.connection_time_till[0x0b] + former_metrics.connection_time_till[0x0b],
            total_connection_time_bucket0c          = client_metrics.connection_time_till[0x0c] + former_metrics.connection_time_till[0x0c],
            total_connection_time_bucket0d          = client_metrics.connection_time_till[0x0d] + former_metrics.connection_time_till[0x0d],
            total_connection_time_bucket0e          = client_metrics.connection_time_till[0x0e] + former_metrics.connection_time_till[0x0e],
            total_connection_time_bucket0f          = client_metrics.connection_time_till[0x0f] + former_metrics.connection_time_till[0x0f],
            total_connection_time_bucket10          = client_metrics.connection_time_till[0x10] + former_metrics.connection_time_till[0x10],
            total_connection_time_bucket11          = client_metrics.connection_time_till[0x11] + former_metrics.connection_time_till[0x11],
            total_connection_time_bucket12          = client_metrics.connection_time_till[0x12] + former_metrics.connection_time_till[0x12],
            total_connection_time_bucket13          = client_metrics.connection_time_till[0x13] + former_metrics.connection_time_till[0x13],
            total_connection_time_bucket14          = client_metrics.connection_time_till[0x14] + former_metrics.connection_time_till[0x14],
            total_connection_time_bucket15          = client_metrics.connection_time_till[0x15] + former_metrics.connection_time_till[0x15],
            total_connection_time_bucket16          = client_metrics.connection_time_till[0x16] + former_metrics.connection_time_till[0x16],
            total_connection_time_bucket17          = client_metrics.connection_time_till[0x17] + former_metrics.connection_time_till[0x17],
            total_connection_time_bucket18          = client_metrics.connection_time_till[0x18] + former_metrics.connection_time_till[0x18],
            total_connection_time_bucket19          = client_metrics.connection_time_till[0x19] + former_metrics.connection_time_till[0x19],
            total_connection_time_bucket1a          = client_metrics.connection_time_till[0x1a] + former_metrics.connection_time_till[0x1a],
            total_connection_time_bucket1b          = client_metrics.connection_time_till[0x1b] + former_metrics.connection_time_till[0x1b],
            total_connection_time_bucket1c          = client_metrics.connection_time_till[0x1c] + former_metrics.connection_time_till[0x1c],
            total_connection_time_bucket1d          = client_metrics.connection_time_till[0x1d] + former_metrics.connection_time_till[0x1d],
            total_connection_time_bucket1e          = client_metrics.connection_time_till[0x1e] + former_metrics.connection_time_till[0x1e],
            total_connection_time_bucket1f          = client_metrics.connection_time_till[0x1f] + former_metrics.connection_time_till[0x1f],
        )
    }

    fn in_client<Func>(
        &self,
        token: &Token,
        action:  Func,
    ) -> Result<(), &'static str>
    where Func: FnOnce(&mut Client) {
        let mut guard = match self.clients.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.len() > token.uid {
            if let Some(ref mut entry) = guard[token.uid] {
                action(entry);
                Ok(())
            } else {
                Err("Already Disconnected")
            }
        } else {
            Err("Invalid Token")
        }
    }

    pub(crate) fn sent_chunk(
        &self,
        token: &Token,
    ) -> Result<(), &'static str> {
        self.in_client(token, |client: &mut Client| client.sent_chunks += 1)
    }

    pub(crate) fn sent_easteregg(
        &self,
        token: &Token,
    ) -> Result<(), &'static str> {
        self.in_client(token, |client: &mut Client| client.sent_eastereggs += 1)
    }

    pub(crate) fn sent_banner(
        &self,
        token: &Token,
    ) -> Result<(), &'static str> {
        self.in_client(token, |client: &mut Client| client.sent_banners += 1)
    }
}

pub(crate) struct Token {
    uid: usize,
}
