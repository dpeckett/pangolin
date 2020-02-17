# Pangolin TODO's

* Respect Prometheus annotations so we can pull metrics on different ports / paths.
* Implement a proper Prometheus metrics parser.
* Average metrics over the full control loop interval rather than pulling once (metrics window parameter?).
* Implement more logging, and expose a prometheus metrics port on Pangolin itself.
* What happens when two or more autoscalers match the same resource? Collision detection?
* Put together an integration test framework for verifying autoscaler behavior.
* Full unit test coverage.
* Put together a CI/CD pipeline.
* Code contribution guidelines etc.
* Implement tunable PID controller.