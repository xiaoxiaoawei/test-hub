# Independent 3D test fixture

`model_asymmetric.step` and `model_asymmetric.obj` are Package Studio's own
asymmetric solid: two overlapping boxes with a cylindrical hole, created using
the public Open CASCADE 8.0 API. They contain no vendor component model.

The fixture exercises analytic curves, pcurves, shared axes, a nonzero centroid,
rotation and translation. An independent OCCT reader is used during release QA
to compare topology, volume, centroid and bounds before/after the Go transform.
The OCCT/Python development environment is not shipped with the application.
