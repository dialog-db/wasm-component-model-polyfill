use alloc::{boxed::Box, vec::Vec};
use core::{
    fmt, mem,
    ops::{Deref, DerefMut},
};

use js_sys::{Function, Map};
use slab::Slab;
use wasm_bindgen::JsValue;
use wasm_runtime_layer::backend::{
    AsContext, AsContextMut, WasmEngine, WasmStore, WasmStoreContext, WasmStoreContextMut,
};

use crate::{
    func::FuncInner, instance::InstanceInner, memory::MemoryInner, table::TableInner, DropResource,
    Engine, Func, Global, GlobalInner, Instance, JsErrorMsg, Memory, Table,
};

/// Owns all the data for the wasm module
///
/// Can be cheaply cloned
///
/// The data is retained through the lifetime of the store, and no GC will collect data from
/// no-longer used modules. It is as such recommended to have the stores lifetime correspond to its
/// modules, and not repeatedly create and drop modules within an existing store, but rather create
/// a new store for it, to avoid unbounded memory use.
pub struct Store<T: 'static> {
    /// The internal store is kept behind a pointer.
    ///
    /// This is to allow referencing and reconstructing a calling context in exported functions,
    /// where it is not possible to prove the correct lifetime and borrowing rules statically nor
    /// dynamically using RefCells. This is because functions can be re-entrant with exclusive but
    /// stacked calling contexts. [`std::cell::RefCell`] and [`std::cell::RefMut`] do not allow
    /// for recursive usage by design (and it would be nigh impossible and quite expensive to enforce at runtime).
    ///
    /// The store is stored through a raw pointer, as using a `Pin<Box<T>>` would not be possible,
    /// despite the memory location of the Box contents technically being pinned in memory. This is
    /// because of the stacked borrows model.
    ///
    /// When the outer box is moved, it invalidates all tags in its borrow stack, even
    /// though the memory location remains. This invalidates all references and raw pointers to `T`
    /// created from the Box.
    ///
    /// See: <https://blog.nilstrieb.dev/posts/box-is-a-unique-type/> for more details.
    ///
    /// By using a box here, we would leave invalid pointers with revoked access permissions to the
    /// memory location of `T`.
    ///
    /// This creates undefined behavior as the Rust compiler will incorrectly optimize register
    /// accesses and memory loading and incorrect no-alias attributes.
    ///
    /// To circumvent this we can use a raw pointer obtained from unwrapping a Box.
    ///
    /// # Playground
    ///
    /// - `Pin<Box<T>>` solution (UB): <https://play.rust-lang.org/?version=stable&mode=debug&edition=2021&gist=685c984584bc0ca1faa780ca292f406c>
    /// - raw pointer solution (sound): <https://play.rust-lang.org/?version=stable&mode=release&edition=2021&gist=257841cb1675106d55c756ad59fde2fb>
    ///
    /// You can use `Tools > Miri` to test the validity
    inner: *mut StoreInner<T>,
}

impl<T: 'static> Store<T> {
    /// Creates a new store from the inner box
    fn from_inner(inner: Box<StoreInner<T>>) -> Self {
        Self {
            inner: Box::into_raw(inner),
        }
    }

    /// Returns a borrow of the store
    pub(crate) fn get(&self) -> StoreContext<'_, T> {
        // Safety:
        //
        // A shared reference to the store signifies a non-mutable ownership, and is thus safe.
        let inner = unsafe { &*self.inner };
        StoreContext::from_ref(inner)
    }

    /// Returns a mutable borrow of the store
    pub(crate) fn get_mut(&mut self) -> StoreContextMut<'_, T> {
        // Safety:
        //
        // &mut self
        let inner = unsafe { &mut *self.inner };
        StoreContextMut::from_ref(inner)
    }
}

impl<T: 'static> Drop for Store<T> {
    fn drop(&mut self) {
        // PATCH (wcmp): a store whose owner asked to retain it stays
        // allocated until `StoreInner::release_orphaned` frees it; see
        // `StoreInner::retain_on_drop`.
        if unsafe { (*self.inner).retained } {
            unsafe { (*self.inner).orphaned = true };
            return;
        }
        unsafe { drop(Box::from_raw(self.inner)) }
    }
}

impl<T: 'static> WasmStore<T, Engine> for Store<T> {
    fn new(engine: &Engine, data: T) -> Self {
        #[cfg(feature = "tracing")]
        let _span = tracing::debug_span!("Store::new").entered();
        Self::from_inner(Box::new(StoreInner {
            engine: engine.clone(),
            instances: Slab::new(),
            funcs: Slab::new(),
            globals: Slab::new(),
            tables: Slab::new(),
            memories: Slab::new(),
            drop_resources: Vec::new(),
            data,
            pending_host_error: None,
            funcref_records: None,
            guest_depth: 0,
            retained: false,
            orphaned: false,
        }))
    }

    fn engine(&self) -> &Engine {
        &self.get().store.engine
    }

    fn data(&self) -> &T {
        &self.get().store.data
    }

    fn data_mut(&mut self) -> &mut T {
        &mut self.get_mut().store.data
    }

    fn into_data(self) -> T {
        // Safety:
        //
        // Ownership of `self` signifies that no guest stack is currently active
        let ptr = unsafe { Box::from_raw(self.inner) };

        // Don't execute drop for `Store`. This impl deallocates the whole box, which we don't
        // want.
        //
        // The box will be deallocated at the end of this scope
        mem::forget(self);

        ptr.data
    }
}

impl<T: 'static> AsContext<Engine> for Store<T> {
    type UserState = T;

    fn as_context(&self) -> <Engine as WasmEngine>::StoreContext<'_, Self::UserState> {
        self.get()
    }
}

impl<T: 'static> AsContextMut<Engine> for Store<T> {
    fn as_context_mut(&mut self) -> StoreContextMut<'_, T> {
        self.get_mut()
    }
}

impl<T: 'static + fmt::Debug> fmt::Debug for Store<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[derive(Debug)]
/// Holds the inner state of the store
pub struct StoreInner<T: 'static> {
    /// The engine used
    pub(crate) engine: Engine,
    /// Instances are not Send + Sync
    pub(crate) instances: Slab<InstanceInner>,
    /// Modules are not Send + Sync
    pub(crate) funcs: Slab<FuncInner>,
    /// Globals
    pub(crate) globals: Slab<GlobalInner>,
    /// Tables
    pub(crate) tables: Slab<TableInner>,
    /// Guest memories
    pub(crate) memories: Slab<MemoryInner>,
    /// The user data
    pub(crate) data: T,

    /// **Note**: append ONLY. No resource must be dropped or removed from this vector as long as
    /// the store is still alive.
    ///
    /// Dropping a resource too early is safe, but the resulting behavior is not specified and may
    /// include incorrect results, memory leaks or panics, etc.
    drop_resources: Vec<DropResource>,

    /// PATCH (wcmp): the error a host function returned during the
    /// current guest call, if any. A host error is thrown into the guest
    /// as a JS exception, which a guest `catch_all` can intercept and
    /// replace (an adapter re-traps with "uncaught exception"). The
    /// native backend surfaces the host's own error on the outer call
    /// because a trap there is not catchable; this slot restores that:
    /// the outer call reports the first host error when the guest fails.
    pub(crate) pending_host_error: Option<anyhow::Error>,

    /// PATCH (wcmp): the function record this store already made for
    /// each function reference it converted, keyed by the JS function
    /// object the reference is.
    ///
    /// A conversion with nothing to look in records a function on
    /// every call and nothing ever removes one, so a store that runs
    /// prepared calls grows without bound. The map is built the first
    /// time a reference is converted or a function exported, so a store
    /// that does neither never allocates it. An exported function's
    /// record is its typed one; see [`StoreInner::remember_exported`].
    funcref_records: Option<Map>,

    /// PATCH (wcmp): how many calls from the host into the guest are
    /// running: a [`Func::call`](crate::Func), or an instantiation
    /// whose start function runs. A promising call is not counted,
    /// because it returns as soon as its stack first suspends.
    pub(crate) guest_depth: usize,

    /// PATCH (wcmp): whether the store stays allocated when its owner
    /// drops it; see [`StoreInner::retain_on_drop`].
    pub(crate) retained: bool,

    /// PATCH (wcmp): whether the owner dropped the store while it was
    /// retained, which leaves it to [`StoreInner::release_orphaned`].
    pub(crate) orphaned: bool,
}

impl<T: 'static> StoreInner<T> {
    /// Inserts a new function and returns its id
    pub(crate) fn insert_func(&mut self, func: FuncInner) -> Func {
        Func {
            id: self.funcs.insert(func),
        }
    }

    /// PATCH (wcmp): the [`Func`] of a function reference the host
    /// received as an argument: the record this store already holds
    /// for that function object, or a new one for a function it has
    /// not seen.
    ///
    /// The JS API hands out one object per function address — the
    /// agent keeps a cache of exported functions and returns the same
    /// object for the same address — so the object is the function's
    /// identity, and one record serves every conversion of it. The
    /// record carries no state of its own, only the function and the
    /// mark that says its signature is unknown, so sharing one
    /// changes nothing a caller can observe.
    ///
    /// A record is never removed. A reference the host received
    /// outlives the call that passed it: the prepare intrinsic keeps
    /// the two functions the adapter generated until the call it
    /// prepared starts, and an asynchronous lower keeps them until
    /// the callee resolves. The store is therefore what bounds their
    /// lifetime, and what this bounds is the count: a store holds one
    /// record per function object it ever saw, so a repeated call
    /// adds none.
    pub(crate) fn func_of_reference(&mut self, function: Function) -> Func {
        let key: &JsValue = function.as_ref();
        if let Some(records) = &self.funcref_records {
            if let Some(id) = records.get(key).as_f64() {
                return Func { id: id as usize };
            }
        }
        let func = self.insert_func(FuncInner::of_unknown_signature(function.clone()));
        self.funcref_records
            .get_or_insert_with(Map::new)
            .set(function.as_ref(), &JsValue::from_f64(func.id as f64));
        func
    }

    /// PATCH (wcmp): make `record`, the record of an instance's export
    /// `function`, the record a reference to that function object
    /// converts to.
    ///
    /// An exported function is a WebAssembly function whose signature
    /// the instance's module declares, and a guest that passes it on
    /// as a `funcref` passes the same function object. With the export
    /// remembered, the host that receives the reference holds the
    /// function with its real signature rather than an unknown one,
    /// which is what a promising call of it needs: JavaScript Promise
    /// Integration runs the function from WebAssembly, and a call from
    /// WebAssembly names the callee's exact type. A function the guest
    /// never exported still converts to a record of unknown
    /// signature.
    pub(crate) fn remember_exported(&mut self, function: &Function, record: &Func) {
        self.funcref_records
            .get_or_insert_with(Map::new)
            .set(function.as_ref(), &JsValue::from_f64(record.id as f64));
    }

    /// PATCH (wcmp): the error a call that ended by throwing `reason`
    /// reports: the error a host function returned during the call,
    /// if one did, and otherwise `reason` itself. Either way the slot
    /// of [`StoreInner::pending_host_error`] is empty afterwards.
    ///
    /// A failed [`Func::call`](crate::Func) and a failed instantiation
    /// report through this. So does the owner of a promising call,
    /// whose failure arrives later, as the rejection of its promise.
    pub fn failure(&mut self, reason: &JsValue) -> anyhow::Error {
        match self.pending_host_error.take() {
            Some(err) => err,
            None => anyhow::Error::from(JsErrorMsg::from(reason)),
        }
    }

    /// PATCH (wcmp): drop the error a host function returned during a
    /// call that went on to succeed, as a [`Func::call`](crate::Func)
    /// that returns does. The owner of a promising call calls this
    /// when the stack the call began suspends or returns, so that a
    /// host error one stack caught never becomes the failure another
    /// stack reports.
    pub fn clear_failure(&mut self) {
        self.pending_host_error = None;
    }

    /// PATCH (wcmp): whether a host function returned an error during
    /// the current guest call, which is then the reason
    /// [`StoreInner::failure`] reports whatever the call threw. The
    /// owner of a promising call whose stack failed before it first
    /// suspended reads this at once, while the rejection of the
    /// promise arrives only on a microtask.
    pub fn pending_failure(&self) -> bool {
        self.pending_host_error.is_some()
    }

    /// PATCH (wcmp): how many calls from the host into the guest are
    /// running now: function calls, and instantiations whose start
    /// function runs.
    ///
    /// A host function that runs inside a stack a promising call
    /// began, at the depth that call was made at, has only
    /// WebAssembly frames between it and the start of that stack, so
    /// a suspending import called above it may suspend the stack. One
    /// that runs deeper has a host frame in between, and a suspension
    /// there traps.
    pub fn guest_depth(&self) -> usize {
        self.guest_depth
    }

    /// PATCH (wcmp): leave the store allocated when its owner drops
    /// it.
    ///
    /// A stack a promising call began resumes on a microtask once its
    /// promise resolves, and the host functions it calls reach the
    /// store through its raw pointer. The owner of such a stack calls
    /// this when the store drops after the promise resolved and
    /// before the stack ran, so that the stack finds the store where
    /// it left it. The stack frees the store with
    /// [`StoreInner::release_orphaned`] once it reached it.
    pub fn retain_on_drop(&mut self) {
        self.retained = true;
    }

    /// PATCH (wcmp): free, on a microtask, a store its owner dropped
    /// while it was retained. It does nothing for a store whose owner
    /// still holds it, or whose release is already queued.
    ///
    /// The stack the store was retained for calls this from a host
    /// function, and reaches the store no more once that host
    /// function returns: it traps, suspends, or ends. All of that
    /// happens before the microtask runs, so no frame holds the store
    /// when it is freed.
    pub fn release_orphaned(&mut self) {
        if !self.orphaned {
            return;
        }
        self.orphaned = false;
        let inner: *mut StoreInner<T> = self;
        // A spawned task first runs on a microtask, never inside the
        // call that spawns it.
        wasm_bindgen_futures::spawn_local(async move {
            // Safety: the owner dropped the store and left it to this
            // microtask, which runs once the stack that queued it no
            // longer reaches it, and nothing else holds it.
            unsafe { drop(Box::from_raw(inner)) }
        });
    }

    /// PATCH (wcmp): how many function records this store holds.
    ///
    /// The count is what says whether a conversion reused a record or
    /// made one, which the test of a repeated call reads.
    pub fn func_count(&self) -> usize {
        self.funcs.len()
    }

    /// Inserts a new global and returns its id
    pub(crate) fn insert_global(&mut self, global: GlobalInner) -> Global {
        Global {
            id: self.globals.insert(global),
        }
    }

    /// Inserts a new table and returns its id
    pub(crate) fn insert_table(&mut self, table: TableInner) -> Table {
        Table {
            id: self.tables.insert(table),
        }
    }

    /// Inserts a new instance and returns its id
    pub(crate) fn insert_instance(&mut self, instance: InstanceInner) -> Instance {
        Instance {
            id: self.instances.insert(instance),
        }
    }

    /// Inserts a new guest memory and returns its id
    pub(crate) fn insert_memory(&mut self, memory: MemoryInner) -> Memory {
        Memory {
            id: self.memories.insert(memory),
        }
    }

    /// Tie the lifetime of a reference or other value to the lifetime of the store using
    /// [`DropResource`].
    pub(crate) fn insert_drop_resource(&mut self, value: DropResource) {
        self.drop_resources.push(value)
    }
}

/// Immutable context to the store
pub struct StoreContext<'a, T: 'static> {
    /// The store
    store: &'a StoreInner<T>,
}

impl<'a, T: 'static> StoreContext<'a, T> {
    /// Provides a store context from a reference
    pub fn from_ref(store: &'a StoreInner<T>) -> Self {
        Self { store }
    }
}

impl<T: 'static> Deref for StoreContext<'_, T> {
    type Target = StoreInner<T>;

    fn deref(&self) -> &Self::Target {
        self.store
    }
}

/// Mutable context to the store
pub struct StoreContextMut<'a, T: 'static> {
    /// The store
    store: &'a mut StoreInner<T>,
}

impl<'a, T: 'static> StoreContextMut<'a, T> {
    /// Returns a pointer to the inner store
    pub(crate) fn as_ptr(&mut self) -> *mut StoreInner<T> {
        self.store as *mut _
    }

    /// Provides a mutable store context from a reference
    pub(crate) fn from_ref(store: &'a mut StoreInner<T>) -> Self {
        Self { store }
    }
}

impl<T: 'static> Deref for StoreContextMut<'_, T> {
    type Target = StoreInner<T>;

    fn deref(&self) -> &Self::Target {
        &*self.store
    }
}

impl<T: 'static> DerefMut for StoreContextMut<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.store
    }
}

impl<'a, T: 'static> WasmStoreContext<'a, T, Engine> for StoreContext<'a, T> {
    fn engine(&self) -> &Engine {
        &self.engine
    }

    fn data(&self) -> &T {
        &self.data
    }
}

impl<'a, T: 'static> AsContext<Engine> for StoreContext<'a, T> {
    type UserState = T;

    fn as_context(&self) -> StoreContext<'_, T> {
        StoreContext { store: self.store }
    }
}

impl<'a, T: 'static> WasmStoreContext<'a, T, Engine> for StoreContextMut<'a, T> {
    fn engine(&self) -> &Engine {
        &self.engine
    }

    fn data(&self) -> &T {
        &self.data
    }
}

impl<'a, T: 'static> WasmStoreContextMut<'a, T, Engine> for StoreContextMut<'a, T> {
    fn data_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<'a, T: 'static> AsContext<Engine> for StoreContextMut<'a, T> {
    type UserState = T;

    fn as_context(&self) -> <Engine as WasmEngine>::StoreContext<'_, T> {
        StoreContext { store: self.store }
    }
}

impl<'a, T: 'static> AsContextMut<Engine> for StoreContextMut<'a, T> {
    fn as_context_mut(&mut self) -> StoreContextMut<'_, T> {
        StoreContextMut { store: self.store }
    }
}
