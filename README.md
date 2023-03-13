# enricher
A basic log enricher

Create log enrichers using the LogEnrichment trait:

```rust
#[derive(Clone)]
struct MacEnricher {}

impl LogEnrichment for MacEnricher {
    fn enrich(&self, mut log: SiemLog, datasets: &DatasetHolder) -> SiemLog {
        let mut fields_to_add = vec![];
        let mac_dataset : &IpMapSynDataset = match datasets.get(&SiemDatasetType::IpMac) {
            Some(dst) => match dst.try_into() {
                Ok(v) => v,
                Err(_) => return log
            },
            None => return log
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

    fn name(&self) -> &str {
        "MacEnricher"
    }

    fn description(&self) -> &str {
        "Adds a Mac to each IP field"
    }
}
```