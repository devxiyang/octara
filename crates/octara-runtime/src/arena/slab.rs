use std::any::{Any, TypeId};

use rustc_hash::FxHashMap;

struct ValueSlot<T> {
    generation: u32,
    value: Option<T>,
}

struct TypedSlab<T> {
    slots: Vec<ValueSlot<T>>,
    free: Vec<u32>,
}

impl<T> TypedSlab<T> {
    fn new() -> Self {
        Self { slots: Vec::new(), free: Vec::new() }
    }

    fn insert(&mut self, generation: u32, value: Option<T>) -> u32 {
        let entry = ValueSlot { generation, value };
        if let Some(index) = self.free.pop() {
            self.slots[index as usize] = entry;
            index
        } else {
            let index = u32::try_from(self.slots.len()).expect("typed slab exhausted");
            assert_ne!(index, u32::MAX, "typed slab exhausted");
            self.slots.push(entry);
            index
        }
    }

    fn slot(&self, index: u32, generation: u32) -> &ValueSlot<T> {
        let slot = &self.slots[index as usize];
        assert_eq!(slot.generation, generation, "invalid typed slab generation");
        slot
    }

    fn slot_mut(&mut self, index: u32, generation: u32) -> &mut ValueSlot<T> {
        let slot = &mut self.slots[index as usize];
        assert_eq!(slot.generation, generation, "invalid typed slab generation");
        slot
    }
}

trait AnySlab: Any {
    fn take_slot(&mut self, index: u32, generation: u32) -> Option<Box<dyn Any>>;
}

impl<T: 'static> AnySlab for TypedSlab<T> {
    fn take_slot(&mut self, index: u32, generation: u32) -> Option<Box<dyn Any>> {
        let value = self.slot_mut(index, generation).value.take();
        self.free.push(index);
        value.map(|value| Box::new(value) as Box<dyn Any>)
    }
}

#[derive(Default)]
pub(super) struct SlabStore {
    by_type: FxHashMap<TypeId, u16>,
    buckets: Vec<Box<dyn AnySlab>>,
}

impl SlabStore {
    pub(super) fn insert<T: 'static>(&mut self, generation: u32, value: Option<T>) -> (u16, u32) {
        let bucket = *self.by_type.entry(TypeId::of::<T>()).or_insert_with(|| {
            let bucket = u16::try_from(self.buckets.len()).expect("typed slab buckets exhausted");
            self.buckets.push(Box::new(TypedSlab::<T>::new()));
            bucket
        });
        (bucket, self.slab_mut::<T>(bucket).insert(generation, value))
    }

    pub(super) fn get<T: 'static>(&self, bucket: u16, index: u32, generation: u32) -> &T {
        let slab: &dyn Any = self.buckets[bucket as usize].as_ref();
        slab.downcast_ref::<TypedSlab<T>>()
            .expect("reactive value type mismatch")
            .slot(index, generation)
            .value
            .as_ref()
            .expect("occupied typed slab slot")
    }

    pub(super) fn get_mut<T: 'static>(&mut self, bucket: u16, index: u32, generation: u32) -> &mut Option<T> {
        &mut self.slab_mut::<T>(bucket).slot_mut(index, generation).value
    }

    pub(super) fn take(&mut self, bucket: u16, index: u32, generation: u32) -> Option<Box<dyn Any>> {
        self.buckets[bucket as usize].take_slot(index, generation)
    }

    fn slab_mut<T: 'static>(&mut self, bucket: u16) -> &mut TypedSlab<T> {
        let slab: &mut dyn Any = self.buckets[bucket as usize].as_mut();
        slab.downcast_mut::<TypedSlab<T>>().expect("reactive value type mismatch")
    }
}
