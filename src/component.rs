use std::borrow::Cow;

use usiem::components::command::{CommandDefinition, SiemFunctionType};
use usiem::components::common::{SiemComponentCapabilities, SiemMessage, UserRole};
use usiem::components::dataset::holder::DatasetHolder;
use usiem::components::enrichment::LogEnrichment;
use usiem::components::metrics::histogram::HistogramVec;
use usiem::components::metrics::{SiemMetric, SiemMetricDefinition};
use usiem::components::storage::SiemComponentStateStorage;
use usiem::components::SiemComponent;
use usiem::crossbeam_channel::{self, TryRecvError};
use usiem::crossbeam_channel::{Receiver, Sender};
use usiem::events::SiemLog;
use usiem::prelude::{
    ComponentError, MessagingError, SiemCommandCall, SiemCommandHeader, SiemDataset, SiemError,
    SiemResult,
};

pub struct LogEnricherComponent {
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
    enrich_metric: HistogramVec,
    metrics: Vec<SiemMetricDefinition>,
}

impl SiemComponent for LogEnricherComponent {
    fn local_channel(&self) -> Sender<SiemMessage> {
        self.local_chnl_snd.clone()
    }

    fn set_log_channel(&mut self, sender: Sender<SiemLog>, receiver: Receiver<SiemLog>) {
        self.log_sender = sender;
        self.log_receiver = receiver;
    }

    fn run(&mut self) -> SiemResult<()> {
        loop {
            self.receive_message()?;
            self.receive_log()?;
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
            Cow::Borrowed("Enrich logs with information extracted from datasets"),
            Cow::Borrowed(""),
            vec![],
            commands,
            vec![],
            self.metrics.clone(),
        )
    }

    fn duplicate(&self) -> Box<dyn SiemComponent> {
        let (local_chnl_snd, local_chnl_rcv) = crossbeam_channel::unbounded();
        Box::new(Self {
            local_chnl_snd,
            enrich_metric: self.enrich_metric.clone(),
            local_chnl_rcv,
            log_receiver: self.log_receiver.clone(),
            log_sender: self.log_sender.clone(),
            datasets: DatasetHolder::new(),
            enrichers: self.enrichers.clone(),
            metrics: self.metrics.clone(),
        })
    }

    fn set_datasets(&mut self, datasets: DatasetHolder) {
        self.datasets = datasets;
    }
}

impl LogEnricherComponent {
    pub fn new(enrichers: &[Box<dyn LogEnrichment>]) -> Self {
        let (local_chnl_snd, local_chnl_rcv) = crossbeam_channel::unbounded();
        let (log_sender, log_receiver) = crossbeam_channel::unbounded();
        let enrichers: Vec<Box<dyn LogEnrichment>> = enrichers.iter().map(|v| v.clone()).collect();
        let (metrics, enrich_metric) = prepare_metrics(&enrichers);

        Self {
            enrichers,
            datasets: DatasetHolder::new(),
            local_chnl_rcv,
            local_chnl_snd,
            log_sender,
            log_receiver,
            metrics,
            enrich_metric,
        }
    }
    pub fn receive_message(&mut self) -> SiemResult<()> {
        let msg = match self.local_chnl_rcv.try_recv() {
            Ok(v) => v,
            Err(e) => match e {
                TryRecvError::Empty => return Ok(()),
                TryRecvError::Disconnected => {
                    println!("Error receive message");
                    return Err(MessagingError::Disconnected.into());
                }
            },
        };
        let res = match msg {
            SiemMessage::Command(hdr, cmd) => self.process_cmd(hdr, cmd),
            SiemMessage::Log(log) => self.process_log(log),
            SiemMessage::Dataset(dataset) => self.process_dataset(dataset),
            _ => Ok(()),
        };
        match res {
            Ok(_) => {}
            Err(e) => match e {
                SiemError::Messaging(msg_e) => match msg_e {
                    MessagingError::Disconnected => return Err(MessagingError::Disconnected.into()),
                    _ => {
                        usiem::error!("Error procesing message in LogEnricher: {:?}", msg_e);
                    }
                },
                _ => {
                    usiem::error!("Error procesing message in LogEnricher: {:?}", e);
                }
            },
        }

        Ok(())
    }
    pub fn receive_log(&mut self) -> SiemResult<()> {
        let log = match self.log_receiver.try_recv() {
            Ok(v) => v,
            Err(e) => match e {
                TryRecvError::Empty => return Ok(()),
                TryRecvError::Disconnected => return Err(MessagingError::Disconnected.into()),
            },
        };
        self.process_log(log)
    }

    fn process_dataset(&mut self, dataset: SiemDataset) -> SiemResult<()> {
        self.datasets.insert(dataset);
        Ok(())
    }
    fn process_cmd(&mut self, _hdr: SiemCommandHeader, cmd: SiemCommandCall) -> SiemResult<()> {
        match cmd {
            SiemCommandCall::STOP_COMPONENT(_n) => {
                usiem::info!("Stopping LogEnricher");
                return Err(ComponentError::StopRequested.into());
            }
            _ => {}
        }
        Ok(())
    }
    fn observe_metric(&self, labels : &[(&str, &str)], val : f64) {
        self.enrich_metric
                .with_labels(labels)
                .and_then(|v| {
                    v.observe(val);
                    Some(v)
                });
    }
    #[cfg(feature="metrics")]
    fn process_log(&mut self, mut log: SiemLog) -> SiemResult<()> {
        let start = coarsetime::Instant::now();
        for enricher in &self.enrichers {
            let now = coarsetime::Instant::now();
            log = enricher.enrich(log, &self.datasets);
            let end = now.elapsed();
            self.observe_metric(&[("name", enricher.name())], end.as_f64());
        }
        let end = start.elapsed();
        self.observe_metric(&[("name", "")], end.as_f64());
        match self.log_sender.send(log) {
            Ok(_) => {}
            Err(_e) => return Err(SiemError::Messaging(MessagingError::Disconnected)),
        }
        
        Ok(())
    }

    #[cfg(not(feature="metrics"))]
    fn process_log(&mut self, mut log: SiemLog) -> SiemResult<()> {
        for enricher in &self.enrichers {
            log = enricher.enrich(log, &self.datasets);
        }
        match self.log_sender.send(log) {
            Ok(_) => {}
            Err(_e) => return Err(SiemError::Messaging(MessagingError::Disconnected)),
        }
        
        Ok(())
    }}

#[cfg(feature="metrics")]
fn prepare_metrics(
    enrichers: &[Box<dyn LogEnrichment>],
) -> (Vec<SiemMetricDefinition>, HistogramVec) {
    let mut ret = Vec::with_capacity(128);
    let mut labels: Vec<Vec<(&str, &str)>> = enrichers.iter().map(|v| vec![("name", v.name())]).collect();
    labels.push(vec![("name", "")]);
    let enrich_metric = HistogramVec::with_le(labels, &[0.000001, 0.0001, 0.01]);
    ret.push(
        SiemMetricDefinition::new(
            "enricher_processed_logs",
            "Processed logs",
            SiemMetric::Histogram(enrich_metric.clone()),
        )
        .unwrap(),
    );
    (ret, enrich_metric)
}
#[cfg(not(feature="metrics"))]
fn prepare_metrics(
    _enrichers: &[Box<dyn LogEnrichment>],
) -> (Vec<SiemMetricDefinition>, HistogramVec) {
    (vec![], HistogramVec::new(vec![]))
}
#[cfg(test)]
mod tst {
    use std::sync::Arc;
    use std::time::Duration;
    use usiem::components::common::SiemMessage;
    use usiem::components::dataset::ip_map::{IpMapDataset, IpMapSynDataset};
    use usiem::components::dataset::{SiemDataset, SiemDatasetType};
    use usiem::components::enrichment::LogEnrichment;
    use usiem::components::metrics::prometheus::Encoder;
    use usiem::components::metrics::{SiemMetric, SiemMetricDefinition};
    use usiem::components::SiemComponent;
    use usiem::crossbeam_channel::{self, Receiver, Sender};
    use usiem::events::field::{SiemField, SiemIp};
    use usiem::events::SiemLog;
    use usiem::prelude::holder::DatasetHolder;
    use usiem::prelude::KernelMessager;
    use usiem::utilities::types::LogString;

    use crate::LogEnricherComponent;

    fn field_name<'a>(name: &'a str) -> &'a str {
        match name.rfind(".") {
            Some(pos) => &name[..pos],
            None => name,
        }
    }

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

    fn prepare_component(names : &[&'static str]) -> (
        Receiver<SiemLog>,
        Sender<SiemLog>,
        Receiver<SiemMessage>,
        Vec<SiemMetricDefinition>,
    ) {
        let (s, receiver) = crossbeam_channel::bounded(10000);
        let (ks, kr) = crossbeam_channel::bounded(10000);
        let (log_sender, log_receiver) = crossbeam_channel::bounded(10000);
        let mut enrichers: Vec<Box<dyn LogEnrichment>> = Vec::with_capacity(128);
        
        for name in names {
            let mac_enricher = MacEnricher {
                name
            };
            enrichers.push(Box::new(mac_enricher));
            
        }
        let mut enricher = LogEnricherComponent::new(&enrichers[..]);
        

        enricher.set_log_channel(s, log_receiver);
        let mut dataset = IpMapDataset::new();
        dataset.insert(SiemIp::V4(100), "SuperData");

        let (comm, _) = crossbeam_channel::bounded(1);
        let datasets = DatasetHolder::from_datasets(vec![SiemDataset::IpMac(
            IpMapSynDataset::new(Arc::new(dataset), comm),
        )]);
        enricher.set_datasets(datasets);
        let metrics = enricher.capabilities().metrics().clone();
        std::thread::spawn(move || {
            usiem::logging::initialize_component_logger(KernelMessager::new(
                123,
                "LogEnricherComponent".to_string(),
                ks,
            ));
            enricher.run()
        });
        (receiver, log_sender, kr, metrics)
    }
    #[test]
    fn enricher_adds_hostname_to_log() {
        let (receiver, sender, _kernel_receiver, _) = prepare_component(&["MacEnricher"]);
        let mut log = SiemLog::new("Simple message", 0, "Testing");
        log.add_field("source.ip", SiemField::IP(SiemIp::V4(100)));
        sender.send(log).expect("Should be sent");
        let log = receiver
            .recv_timeout(Duration::from_millis(200))
            .expect("Should receive a message");
        assert!(log.has_field("source.mac"));
        assert_eq!(
            log.field("source.mac"),
            Some(&SiemField::from_str("SuperData"))
        );
    }

    #[cfg(feature="metrics")]
    #[test]
    fn enricher_updates_metrics() {
        let (receiver, sender, _kernel_receiver, metrics) = prepare_component(&["MacEnricher"]);
        let joined = std::thread::spawn(move || {
            for _ in 0..1_000_000 {
                receiver.recv().unwrap();
            }
        });
        for _ in 0..1_000_000 {
            let mut log = SiemLog::new("Simple message", 0, "Testing");
            log.add_field("source.ip", SiemField::IP(SiemIp::V4(100)));
            sender.send(log).expect("Should be sent");
        }
        drop(sender);
        joined.join().unwrap();
        let metric = metrics.get(0).unwrap();
        match metric.metric() {
            SiemMetric::Histogram(metric) => {
                let metric = metric.with_labels(&[("name", "MacEnricher")]).unwrap();
                assert_eq!(1_000_000, metric.get_sample_count());
                println!("Metric sum: {}", metric.get_sample_sum());
                println!(
                    "Metric median {}",
                    (metric.get_sample_sum() as f64) / (metric.get_sample_count() as f64)
                )
            }
            _ => unreachable!(),
        }
    }


    #[cfg(feature="metrics")]
    #[test]
    fn enricher_generates_metrics_with_labels() {
        let (receiver, sender, _kernel_receiver, metrics) = prepare_component(&["Enricher1", "Enricher2", "Enricher3", "Enricher4", "Enricher5"]);
        let joined = std::thread::spawn(move || {
            for _ in 0..100_000 {
                receiver.recv().unwrap();
            }
        });
        for _ in 0..100_000 {
            let mut log = SiemLog::new("Simple message", 0, "Testing");
            log.add_field("source.ip", SiemField::IP(SiemIp::V4(100)));
            sender.send(log).expect("Should be sent");
        }
        drop(sender);
        joined.join().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let metric_def = metrics.get(0).unwrap();
        match metric_def.metric() {
            SiemMetric::Histogram(metric) => {
                let hist = metric.with_labels(&[("name", "Enricher1")]).unwrap();
                assert_eq!(100_000, hist.get_sample_count());
                let hist = metric.with_labels(&[("name", "Enricher2")]).unwrap();
                assert_eq!(100_000, hist.get_sample_count());
                let hist = metric.with_labels(&[("name", "Enricher3")]).unwrap();
                assert_eq!(100_000, hist.get_sample_count());
                let hist = metric.with_labels(&[("name", "Enricher4")]).unwrap();
                assert_eq!(100_000, hist.get_sample_count());
                let hist = metric.with_labels(&[("name", "Enricher5")]).unwrap();
                assert_eq!(100_000, hist.get_sample_count());
                let mut st = String::with_capacity(1_0000);
                metric.encode(&mut st, metric_def.name(), metric_def.description(), true).unwrap();
                println!("{}", st);
            }
            _ => unreachable!(),
        }
        
    }

    #[cfg(not(feature="metrics"))]
    #[test]
    fn enricher_does_not_generate_metrics() {
        let (_receiver, _sender, _kernel_receiver, metrics) = prepare_component(&["Enricher1"]);
        assert_eq!(0, metrics.len());
    }

}
