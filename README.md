# enricher
A basic log enricher

Create log enrichers using the LogEnrichment trait:

```rust
#[derive(Clone)]
struct MacEnricher {}

impl LogEnrichment for MacEnricher {
    fn enrich(
        &self,
        mut log: SiemLog,
        datasets: &usiem::components::dataset::holder::DatasetHolder,
    ) -> SiemLog {
        let mut fields_to_add = vec![];
        for (name, field) in log.fields() {
            if let SiemField::IP(ip) = field {
                let ip_mac = match datasets.get(&SiemDatasetType::IpMac) {
                    Some(ip_mac) => match ip_mac {
                        SiemDataset::IpMac(ip_mac) => ip_mac,
                        _ => {
                            continue;
                        }
                    },
                    None => {
                        continue;
                    }
                };
                match ip_mac.get(ip) {
                    Some(val) => {
                        let field_base_name = field_name(name);
                        fields_to_add.push((format!("{}.mac", field_base_name),SiemField::Text(val.clone())));
                    }
                    None => {}
                }
            }
        }
        for (name,val) in fields_to_add {
            log.add_field(&name, val);
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