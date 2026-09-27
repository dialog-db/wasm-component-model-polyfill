/// One thing a developer does with the polyfill in their own project,
/// as the smoke test tells it: the chapter it is told under, what the
/// developer sets out to do, and the situation they are in.
#[derive(Debug)]
pub struct Story {
    /// The chapter of the report the story is told under.
    pub chapter: &'static str,
    /// What the developer sets out to do, as an imperative phrase.
    pub title: &'static str,
    /// What the developer does and what they expect, in one short
    /// sentence. A second short sentence may say how the story goes
    /// in a browser without JavaScript Promise Integration (JSPI).
    pub goal: &'static str,
}
