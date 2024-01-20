use indexmap::IndexSet;
use typst_ts_core::debug_loc::SourceSpan;

pub enum InternQuery<T> {
    Ok(Option<T>),
    UseAfterFree,
}

pub struct InternId {
    lifetime: u32,
    id: u32,
}

impl InternId {
    pub fn new(lifetime: usize, id: usize) -> Self {
        Self {
            lifetime: lifetime as u32,
            id: id as u32,
        }
    }

    fn to_u64(&self) -> u64 {
        (self.lifetime as u64) << 32 | self.id as u64
    }

    fn from_u64(id: u64) -> Self {
        Self {
            lifetime: (id >> 32) as u32,
            id: (id & 0xffffffff) as u32,
        }
    }

    pub fn to_hex(&self) -> String {
        format!("{:x}", self.to_u64())
    }

    pub fn from_hex(hex: &str) -> Self {
        Self::from_u64(u64::from_str_radix(hex, 16).unwrap())
    }
}

pub struct SpanInterner {
    lifetime: usize,
    span2id: IndexSet<(usize, SourceSpan)>,
}

impl Default for SpanInterner {
    fn default() -> Self {
        Self::new()
    }
}

const GARAGE_COLLECT_THRESHOLD: usize = 30;

impl SpanInterner {
    pub fn new() -> Self {
        Self {
            lifetime: 1,
            span2id: IndexSet::new(),
        }
    }

    pub fn reset(&mut self) {
        self.lifetime += 1;
        self.span2id
            .retain(|(id, _)| self.lifetime - id < GARAGE_COLLECT_THRESHOLD);
    }

    pub fn span_by_str(&self, str: &str) -> InternQuery<&SourceSpan> {
        self.span(InternId::from_hex(str))
    }

    pub fn span(&self, id: InternId) -> InternQuery<&SourceSpan> {
        if (id.lifetime as usize + GARAGE_COLLECT_THRESHOLD) <= self.lifetime {
            InternQuery::UseAfterFree
        } else {
            InternQuery::Ok(self.span2id.get_index(id.id as usize).map(|(_, span)| span))
        }
    }

    pub fn intern(&mut self, span: SourceSpan) -> InternId {
        let item = (self.lifetime, span);
        let (idx, _) = self.span2id.insert_full(item);
        // combine lifetime

        InternId::new(self.lifetime, idx)
    }
}
