#[derive(Clone, Copy, Debug)]
pub struct SamplePolicy {
    pub limit: usize,
    pub bytes: usize,
    pub max_doc_bytes: usize,
    pub all: bool,
}

impl Default for SamplePolicy {
    fn default() -> Self {
        Self {
            limit: 500,
            bytes: 64 * 1024 * 1024,
            max_doc_bytes: 4 * 1024 * 1024,
            all: false,
        }
    }
}

impl SamplePolicy {
    #[must_use]
    pub fn from_env() -> Self {
        let mut policy = Self::default();
        if let Some(value) = env_usize("SIMDHTML_BENCH_LIMIT") {
            policy.limit = value.max(1);
        }
        if let Some(value) = env_usize("SIMDHTML_BENCH_BYTES") {
            policy.bytes = value.max(1);
        }
        if let Some(value) = env_usize("SIMDHTML_BENCH_MAX_DOC_BYTES") {
            policy.max_doc_bytes = value.max(1);
        }
        policy.all = std::env::var_os("SIMDHTML_BENCH_ALL").is_some_and(|value| value != "0");
        policy
    }

    #[must_use]
    pub fn quota(&self, bucket_count: usize) -> crate::Quota {
        let buckets = bucket_count.max(1);
        if self.all {
            return crate::Quota {
                docs: usize::MAX,
                bytes: usize::MAX,
            };
        }
        crate::Quota {
            docs: self.limit.div_ceil(buckets).max(1),
            bytes: (self.bytes / buckets).max(1),
        }
    }

    #[must_use]
    pub fn describe(&self) -> String {
        if self.all {
            "all documents".to_string()
        } else {
            format!("{} docs / {} MiB", self.limit, self.bytes / (1024 * 1024))
        }
    }
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
}
