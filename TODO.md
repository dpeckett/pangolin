# Pangolin TODO

* Respect Prometheus annotations so we can pull metrics on different ports / paths.
* Implement a proper Prometheus metrics parser.
* Expose a prometheus metrics port on Pangolin itself.
* What happens when two or more autoscalers match the same resource? Collision detection?
* Put together an integration test framework for verifying autoscaler behavior.
* Unit test coverage for `main.rs`?.
* Put together a CI/CD pipeline.
* Code contribution guidelines etc.
* Implement tunable PID controller.