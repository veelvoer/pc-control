use proto::{Disk, Proc, Stats};
use sysinfo::{Components, Disks, Networks, ProcessesToUpdate, System};

pub struct Sampler {
    sys: System,
    nets: Networks,
    disks: Disks,
    comps: Components,
    tick: u32,
    top: Vec<Proc>,
}

impl Sampler {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        Self { sys, nets: Networks::new_with_refreshed_list(), disks: Disks::new_with_refreshed_list(), comps: Components::new_with_refreshed_list(), tick: 0, top: vec![] }
    }

    /// Call once per second.
    pub fn sample(&mut self) -> Stats {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.nets.refresh(true);
        self.tick += 1;
        if self.tick % 2 == 1 {
            self.sys.refresh_processes(ProcessesToUpdate::All, true);
            let mut v: Vec<Proc> = self.sys.processes().values()
                .map(|p| Proc { name: p.name().to_string_lossy().into_owned(), cpu: p.cpu_usage(), mem: p.memory() })
                .collect();
            v.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
            v.truncate(5);
            self.top = v;
        }
        if self.tick % 10 == 1 {
            self.disks.refresh(true);
            self.comps.refresh(true);
        }
        let (rx, tx) = self.nets.iter().fold((0, 0), |(r, t), (_, n)| (r + n.received(), t + n.transmitted()));
        let temp = self.comps.iter().filter_map(|c| c.temperature()).filter(|t| t.is_finite()).reduce(f32::max);
        let la = System::load_average();
        Stats {
            cpu: self.sys.global_cpu_usage(),
            cpu_cores: self.sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            swap_used: self.sys.used_swap(),
            swap_total: self.sys.total_swap(),
            net_rx_bps: rx,
            net_tx_bps: tx,
            disks: self.disks.iter()
                .filter(|d| d.total_space() > 0 && !d.mount_point().starts_with("/boot") && !d.mount_point().starts_with("/var/lib"))
                .map(|d| Disk { mount: d.mount_point().to_string_lossy().into_owned(), used: d.total_space() - d.available_space(), total: d.total_space() })
                .collect(),
            temp_c: temp,
            uptime_s: System::uptime(),
            load: [la.one as f32, la.five as f32, la.fifteen as f32],
            top: self.top.clone(),
        }
    }
}
