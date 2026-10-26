//! Tests of the unwrapper, and the meshes and checks they share.
//!
//! * `meshes`: the test models, built here (spheres, cubes, a torus, a holed
//!   grid), as triangles and as quads.
//! * `checks`: what every unwrap must satisfy, measured: UVs in the square,
//!   no two charts overlapping, every chart one piece, nothing flipped.
//! * `shapes`: each test model unwrapped and checked, with its numbers
//!   printed (`--nocapture`).
//! * `seams`: quads kept whole, face groups kept apart.
//! * `refusals`: what is refused, and the broken meshes that are not.
//! * `timing`: a model of the size the app unwraps (`--ignored`).

mod checks;
mod meshes;
mod refusals;
mod seams;
mod shapes;
mod timing;
