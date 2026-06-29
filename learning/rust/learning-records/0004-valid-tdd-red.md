<!-- Boundary: Records demonstrated understanding of test discovery and valid TDD failure; it does not claim general testing mastery. -->
# A valid TDD red must exercise the test

The learner correctly distinguished “0 tests run” caused by a missing `mod tests;` declaration from the later compile failure caused by the genuinely missing `HttpTransportHandle`. Future TDD steps can assume they know that a red result is useful only after the intended test is actually registered and exercised.
