use std::borrow::Cow;

use usiem::components::command::{CommandDefinition, SiemCommandCall, SiemFunctionType};
use usiem::components::common::{
    SiemComponentCapabilities, SiemComponentStateStorage, SiemMessage, UserRole,
};
use usiem::components::dataset::holder::DatasetHolder;
use usiem::components::dataset::SiemDataset;
use usiem::components::enrichment::LogEnrichment;
use usiem::components::SiemComponent;
use usiem::crossbeam_channel;
use usiem::crossbeam_channel::{Receiver, Sender, TryRecvError};
use usiem::events::SiemLog;

pub struct LogEnricher {
    id: u64,
    /// Send messages to the kernel
    kernel_sender: Sender<SiemMessage>,
    /// Receive actions from other components or the kernel
    local_chnl_rcv: Receiver<SiemMessage>,
    /// Send actions to this components
    local_chnl_snd: Sender<SiemMessage>,
    /// Send logs to the next component
    log_sender: Sender<SiemLog>,
    /// Recive logs from the previous component
    log_receiver: Receiver<SiemLog>,
    datasets: DatasetHolder,
    enrichers: Vec<Box<dyn LogEnrichment>>,
}

impl SiemComponent for LogEnricher {
    fn set_id(&mut self, id: u64) {
        self.id = id;
    }

    fn local_channel(&self) -> Sender<SiemMessage> {
        self.local_chnl_snd.clone()
    }

    fn set_log_channel(&mut self, sender: Sender<SiemLog>, receiver: Receiver<SiemLog>) {
        self.log_sender = sender;
        self.log_receiver = receiver;
    }

    fn set_kernel_sender(&mut self, sender: Sender<SiemMessage>) {
        self.kernel_sender = sender;
    }

    fn run(&mut self) {
        let receiver = self.local_chnl_rcv.clone();
        let log_receiver = self.log_receiver.clone();

        let mut datasets = self.datasets.clone();
        loop {
            let rcv_action = receiver.try_recv();
            match rcv_action {
                Ok(msg) => match msg {
                    SiemMessage::Command(_hdr, cmd) => match cmd {
                        SiemCommandCall::STOP_COMPONENT(_n) => {
                            println!("Closing LogEnricher");
                            return;
                        }
                        _ => {}
                    },
                    SiemMessage::Log(mut log) => {
                        for enricher in &self.enrichers {
                            log = enricher.enrich(log, &datasets);
                        }
                        loop {
                            match self.log_sender.send(log) {
                                Ok(_) => {
                                    break;
                                }
                                Err(e) => {
                                    log = e.0;
                                }
                            }
                        }
                    }
                    SiemMessage::Dataset(dataset) => {
                        datasets.add(dataset);
                    }
                    _ => {}
                },
                Err(e) => match e {
                    TryRecvError::Empty => {}
                    TryRecvError::Disconnected => {
                        return;
                    }
                },
            };

            match log_receiver.try_recv() {
                Ok(mut log) => {
                    for enricher in &self.enrichers {
                        log = enricher.enrich(log, &datasets);
                    }
                    loop {
                        match self.log_sender.send(log) {
                            Ok(_) => {
                                break;
                            }
                            Err(e) => {
                                log = e.0;
                            }
                        }
                    }
                }
                Err(e) => match e {
                    TryRecvError::Empty => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    }
                    TryRecvError::Disconnected => return,
                },
            }
        }
    }

    fn set_storage(&mut self, _conn: Box<dyn SiemComponentStateStorage>) {}

    fn capabilities(&self) -> SiemComponentCapabilities {
        let commands = vec![
            CommandDefinition::new(
                SiemFunctionType::STOP_COMPONENT,
                Cow::Borrowed("Stops enricher component"),
                Cow::Borrowed("Stops the LogEnricher component"),
                UserRole::Administrator,
            ),
            CommandDefinition::new(
                SiemFunctionType::START_COMPONENT, // Must be added by default by the KERNEL and only used by him
                Cow::Borrowed("Starts enricher component"),
                Cow::Borrowed("Starts the LogEnricher component"),
                UserRole::Administrator,
            ),
        ];
        SiemComponentCapabilities::new(
            Cow::Borrowed("LogEnricher"),
            Cow::Borrowed("ENrich logs with information extracted from datasets"),
            Cow::Borrowed(""),
            vec![],
            commands,
            vec![],
            vec![],
        )
    }

    fn duplicate(&self) -> Box<dyn SiemComponent> {
        let (local_chnl_snd, local_chnl_rcv) = crossbeam_channel::unbounded();
        Box::new(Self {
            id: 0,
            kernel_sender: self.kernel_sender.clone(),
            local_chnl_snd,
            local_chnl_rcv,
            log_receiver: self.log_receiver.clone(),
            log_sender: self.log_sender.clone(),
            datasets: DatasetHolder::new(),
            enrichers: self.enrichers.clone(),
        })
    }

    fn set_datasets(&mut self, datasets: Vec<SiemDataset>) {
        for dataset in datasets {
            self.datasets.add(dataset);
        }
    }
}

impl LogEnricher {
    pub fn new() -> Self {
        let (kernel_sender, _receiver) = crossbeam_channel::bounded(1000);
        let (local_chnl_snd, local_chnl_rcv) = crossbeam_channel::unbounded();
        let (log_sender, log_receiver) = crossbeam_channel::unbounded();
        Self {
            id: 0,
            enrichers: vec![],
            datasets: DatasetHolder::new(),
            kernel_sender,
            local_chnl_rcv,
            local_chnl_snd,
            log_sender,
            log_receiver,
        }
    }

    pub fn add_enricher(&mut self, enricher: Box<dyn LogEnrichment>) {
        self.enrichers.push(enricher);
    }
}

#[cfg(test)]
mod elastic_test {
    use std::sync::Arc;
    use usiem::components::command::{SiemCommandCall, SiemCommandHeader};
    use usiem::components::common::SiemMessage;
    use usiem::components::dataset::ip_map::{IpMapDataset, IpMapSynDataset};
    use usiem::components::dataset::{SiemDataset, SiemDatasetType};
    use usiem::components::enrichment::LogEnrichment;
    use usiem::components::SiemComponent;
    use usiem::crossbeam_channel;
    use usiem::events::field::{SiemField, SiemIp};
    use usiem::events::{SiemLog};

    use crate::LogEnricher;

    fn field_name<'a>(name: &'a str) -> &'a str {
        match name.rfind(".") {
            Some(pos) => &name[..pos],
            None => name,
        }
    }

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

    #[test]
    fn enricher_adds_hostname_to_log() {
        let (s, receiver) = crossbeam_channel::bounded(10);
        let (log_sender, log_receiver) = crossbeam_channel::bounded(10);

        let mut enricher = LogEnricher::new();
        enricher.set_log_channel(s, log_receiver);
        let local_channel = enricher.local_channel();
        let mut dataset = IpMapDataset::new();
        dataset.insert(SiemIp::V4(100), "SuperData");

        let (comm, _) = crossbeam_channel::bounded(1);
        let dataset = SiemDataset::IpMac(IpMapSynDataset::new(Arc::new(dataset), comm));

        enricher.set_datasets(vec![dataset]);

        let mac_enricher = MacEnricher {};
        enricher.add_enricher(Box::new(mac_enricher));

        let mut log = SiemLog::new("Simple message", 0, "Testing");
        log.add_field("source.ip", SiemField::IP(SiemIp::V4(100)));
        log_sender.send(log).expect("Should be sent");

        std::thread::spawn(move || enricher.run());
        std::thread::sleep(std::time::Duration::from_millis(100));
        let log = receiver.try_recv().expect("Should receive a message");
        assert!(log.has_field("source.mac"));
        assert_eq!(log.field("source.mac"), Some(&SiemField::from_str("SuperData")));

        std::thread::spawn(move || {
            local_channel.send(SiemMessage::Command(
                SiemCommandHeader {
                    comm_id: 0,
                    comp_id: 0,
                    user: "paco".to_string(),
                },
                SiemCommandCall::STOP_COMPONENT("STOP!".to_string()),
            ))
        });
    }
}
