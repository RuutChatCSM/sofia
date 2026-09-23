# Windows exec-server fixture

This directory contains the small Windows exec-server binary used by
foreign-OS tests. It links only `sofia-exec-server` because the full Sofia
Windows graph does not yet cross-build with Bazel.
