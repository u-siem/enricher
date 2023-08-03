# µSIEM Enricher
[![Documentation](https://docs.rs/u-siem-enricher/badge.svg)](https://docs.rs/u-siem-enricher) ![crates.io](https://img.shields.io/crates/v/u-siem-enricher.svg)

A basic log enricher component.

## Usage

```rust
let mut enrichers: Vec<Box<dyn LogEnrichment>> = Vec::with_capacity(128);  
let mac_enricher = MacEnricher {
    name : "MacEnricher"
};
enrichers.push(Box::new(mac_enricher));
let mut enricher = LogEnricherComponent::new(&enrichers[..]);
```

## Metrics
Enabled by default using the `metrics` feature.
```
# HELP enricher_processed_logs Processed logs
# TYPE enricher_processed_logs histogram 
enricher_processed_logs_bucket{name="Enricher1",le=0.000001} 99948
enricher_processed_logs_bucket{name="Enricher1",le=0.0001} 99948
enricher_processed_logs_bucket{name="Enricher1",le=0.01} 100000
enricher_processed_logs_sum{name="Enricher1"} 0.208
enricher_processed_logs_count{name="Enricher1"} 100000
enricher_processed_logs_bucket{name="Enricher2",le=0.000001} 99959
enricher_processed_logs_bucket{name="Enricher2",le=0.0001} 99959
enricher_processed_logs_bucket{name="Enricher2",le=0.01} 100000
enricher_processed_logs_sum{name="Enricher2"} 0.164
enricher_processed_logs_count{name="Enricher2"} 100000
enricher_processed_logs_bucket{name="Enricher3",le=0.000001} 99963
enricher_processed_logs_bucket{name="Enricher3",le=0.0001} 99963
enricher_processed_logs_bucket{name="Enricher3",le=0.01} 100000
enricher_processed_logs_sum{name="Enricher3"} 0.148
enricher_processed_logs_count{name="Enricher3"} 100000
enricher_processed_logs_bucket{name="Enricher4",le=0.000001} 99953
enricher_processed_logs_bucket{name="Enricher4",le=0.0001} 99953
enricher_processed_logs_bucket{name="Enricher4",le=0.01} 100000
enricher_processed_logs_sum{name="Enricher4"} 0.188
enricher_processed_logs_count{name="Enricher4"} 100000
enricher_processed_logs_bucket{name="Enricher5",le=0.000001} 99968
enricher_processed_logs_bucket{name="Enricher5",le=0.0001} 99968
enricher_processed_logs_bucket{name="Enricher5",le=0.01} 100000
enricher_processed_logs_sum{name="Enricher5"} 0.128
enricher_processed_logs_count{name="Enricher5"} 100000
enricher_processed_logs_bucket{le=0.000001} 99725
enricher_processed_logs_bucket{le=0.0001} 99725
enricher_processed_logs_bucket{le=0.01} 100000
enricher_processed_logs_sum{} 1.1
enricher_processed_logs_count{} 100000
```

## Enricher examples

Create log enrichers using the LogEnrichment trait:

```rust
#[derive(Clone)]
struct MacEnricher {
    pub name : &'static str
}

impl LogEnrichment for MacEnricher {
    fn enrich(&self, mut log: SiemLog, datasets: &DatasetHolder) -> SiemLog {
        let mut fields_to_add = vec![];
        let mac_dataset: &IpMapSynDataset = match datasets.get(&SiemDatasetType::IpMac) {
            Some(dst) => match dst.try_into() {
                Ok(v) => v,
                Err(_) => return log,
            },
            None => return log,
        };

        for (name, field) in log.fields() {
            if let SiemField::IP(ip) = field {
                match mac_dataset.get(ip) {
                    Some(val) => {
                        fields_to_add.push((
                            format!("{}.mac", field_name(name)),
                            SiemField::Text(val.clone()),
                        ));
                    }
                    None => {}
                }
            }
        }
        for (name, val) in fields_to_add {
            log.insert(LogString::Owned(name), val);
        }
        log
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        "Adds a Mac to each IP field"
    }
}
```