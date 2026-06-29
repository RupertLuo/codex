<!-- Boundary: Records demonstrated understanding of a test's behavioral limits; it does not claim complete TDD mastery. -->
# Tests prove only observed behavior

The learner correctly explained that the constructor-and-clone test passed even though `new` discarded both closures because the test never invoked either closure. Future steps can build behavior incrementally by adding an assertion that observes each required effect before expanding the implementation.
