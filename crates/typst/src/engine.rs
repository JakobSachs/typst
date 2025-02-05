use std::cell::Cell;

use comemo::{Track, Tracked, TrackedMut, Validate};

use crate::diag::SourceResult;
use crate::eval::Tracer;
use crate::introspection::{Introspector, Locator};
use crate::syntax::FileId;
use crate::World;

/// The maxmium stack nesting depth.
const MAX_DEPTH: usize = 64;

/// Holds all data needed during compilation.
pub struct Engine<'a> {
    /// The compilation environment.
    pub world: Tracked<'a, dyn World + 'a>,
    /// Provides access to information about the document.
    pub introspector: Tracked<'a, Introspector>,
    /// The route the engine took during compilation. This is used to detect
    /// cyclic imports and too much nesting.
    pub route: Route<'a>,
    /// Provides stable identities to elements.
    pub locator: &'a mut Locator<'a>,
    /// The tracer for inspection of the values an expression produces.
    pub tracer: TrackedMut<'a, Tracer>,
}

impl Engine<'_> {
    /// Perform a fallible operation that does not immediately terminate further
    /// execution. Instead it produces a delayed error that is only promoted to
    /// a fatal one if it remains at the end of the introspection loop.
    pub fn delayed<F, T>(&mut self, f: F) -> T
    where
        F: FnOnce(&mut Self) -> SourceResult<T>,
        T: Default,
    {
        match f(self) {
            Ok(value) => value,
            Err(errors) => {
                self.tracer.delay(errors);
                T::default()
            }
        }
    }
}

/// The route the engine took during compilation. This is used to detect
/// cyclic imports and too much nesting.
#[derive(Clone)]
pub struct Route<'a> {
    // We need to override the constraint's lifetime here so that `Tracked` is
    // covariant over the constraint. If it becomes invariant, we're in for a
    // world of lifetime pain.
    outer: Option<Tracked<'a, Self, <Route<'static> as Validate>::Constraint>>,
    /// This is set if this route segment was inserted through the start of a
    /// module evaluation.
    id: Option<FileId>,
    /// This is set whenever we enter a function, nested layout, or are applying
    /// a show rule. The length of this segment plus the lengths of all `outer`
    /// route segments make up the length of the route. If the length of the
    /// route exceeds `MAX_DEPTH`, then we throw a "maximum ... depth exceeded"
    /// error.
    len: usize,
    /// The upper bound we've established for the parent chain length. We don't
    /// know the exact length (that would defeat the whole purpose because it
    /// would prevent cache reuse of some computation at different,
    /// non-exceeding depths).
    upper: Cell<usize>,
}

impl<'a> Route<'a> {
    /// Create a new, empty route.
    pub fn root() -> Self {
        Self { id: None, outer: None, len: 0, upper: Cell::new(0) }
    }

    /// Insert a new id into the route.
    ///
    /// You must guarantee that `outer` lives longer than the resulting
    /// route is ever used.
    pub fn insert(outer: Tracked<'a, Self>, id: FileId) -> Self {
        Route {
            outer: Some(outer),
            id: Some(id),
            len: 0,
            upper: Cell::new(usize::MAX),
        }
    }

    /// Extend the route without another id.
    pub fn extend(outer: Tracked<'a, Self>) -> Self {
        Route {
            outer: Some(outer),
            id: None,
            len: 1,
            upper: Cell::new(usize::MAX),
        }
    }

    /// Start tracking this route.
    ///
    /// In comparison to [`Track::track`], this method skips this chain link
    /// if it does not contribute anything.
    pub fn track(&self) -> Tracked<'_, Self> {
        match self.outer {
            Some(outer) if self.id.is_none() && self.len == 0 => outer,
            _ => Track::track(self),
        }
    }

    /// Increase the nesting depth for this route segment.
    pub fn increase(&mut self) {
        self.len += 1;
    }

    /// Decrease the nesting depth for this route segment.
    pub fn decrease(&mut self) {
        self.len -= 1;
    }

    /// Check whether the nesting depth exceeds the limit.
    pub fn exceeding(&self) -> bool {
        !self.within(MAX_DEPTH)
    }
}

#[comemo::track]
impl<'a> Route<'a> {
    /// Whether the given id is part of the route.
    pub fn contains(&self, id: FileId) -> bool {
        self.id == Some(id) || self.outer.map_or(false, |outer| outer.contains(id))
    }

    /// Whether the route's depth is less than or equal to the given depth.
    pub fn within(&self, depth: usize) -> bool {
        if self.upper.get().saturating_add(self.len) <= depth {
            return true;
        }

        match self.outer {
            Some(_) if depth < self.len => false,
            Some(outer) => {
                let within = outer.within(depth - self.len);
                if within && depth < self.upper.get() {
                    self.upper.set(depth);
                }
                within
            }
            None => true,
        }
    }
}

impl Default for Route<'_> {
    fn default() -> Self {
        Self::root()
    }
}
