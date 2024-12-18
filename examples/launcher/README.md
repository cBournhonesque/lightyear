# Server launcher for docker image

For hosting the example servers, all the server binaries are in a single docker image.
This launcher is the docker entrypoint that decides which one to run, based on $EXAMPLE_NAME env var.

This could be a bash script:
```bash
#!/bin/bash
exec /apps/$EXAMPLE_NAME/$EXAMPLE_NAME server
```

## No shell

However... in order to keep the resulting docker image as tiny as possible, so deployment is as fast as possible, it's using an absolute bare-bones docker image with just our rust binaries copied in.
https://github.com/GoogleContainerTools/distroless

it doesn't include a shell like bash or sh. there is a version that includes a shell, but that adds more size

## But.. ENTRYPOINT?

Normally you'd just set the docker ENTRYPOINT when launching the container, depending on which example, to say "/apps/spaceships/spaceships" and not need a launcher, but that's not exposed via the edgegap dashboard, and setting envs is. if they add entry point settings to the dashboard we can delete the launcher.