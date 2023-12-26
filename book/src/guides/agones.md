# Agones

Here are my notes about integration with Agones.

Agones is a game server hosting platform built on top of Kubernetes. It allows you to run game servers on demand, and scale them up and down as needed.
It has a centralized control plane that monitors the health of the nodes and game servers (one node might host multiple game servers).



### Step-by-step

- Rent a VM (I used kamatera)
  - needs at least 2CPUs, 2GB RAM, 20GB disk
- Install docker using this page: https://docs.docker.com/engine/install/ubuntu/#install-using-the-repository
- Install minikube with: https://minikube.sigs.k8s.io/docs/start/
- Set minikube to use docker: `minikube config set driver docker`
- Start minikube with `minikube start --force` to check that it works
  - I used `--force` to start minikube with root user, alternatively you could create another user ([stackoverflow](https://stackoverflow.com/questions/68984450/minikube-why-the-docker-driver-should-not-be-used-with-root-privileges))
    - if so, I also had to run `sudo usermod -aG docker $USER && newgrp docker` afterwards
  - If you don't have `kubectl` installed, you can re-use minikube's kubectl: `ln -s $(which minikube) /usr/local/bin/kubectl` (see [this](https://minikube.sigs.k8s.io/docs/handbook/kubectl/))
  - Works!
- Start a minikube cluster for agones: [link](https://agones.dev/site/docs/installation/creating-cluster/minikube/)
  - Start a minikube cluster with a version compatible with agones, and the "agones" profile: `minikube start --kubernetes-version v1.27.6 -p agones`
  - At this point I kept running into [this](https://github.com/kubernetes/minikube/issues/14185) so I decided to install docker desktop ([link](https://docs.docker.com/desktop/install/ubuntu/#install-docker-desktop))
  - It didn't work; I tried running `sudo minikube delete -p agones` and retrying
  - then I need to tell minikube to use that profile: `minikube profile agones`
  - this time it worked! (check with `kubectl get po -A`)
- Run `minikube ip -p agones` to get the local IP of the minikube cluster
- Install helm: [link](https://helm.sh/docs/intro/install/#from-apt-debianubuntu)
- Install agones: [link](https://agones.dev/site/docs/installation/install-agones/)
  - install the helm chart: [link](https://agones.dev/site/docs/installation/install-agones/helm/#installing-the-chart)
  - installs correctly!
- Install k9s: 
  - see release binary (.tar.gz) in https://github.com/derailed/k9s/releases
  - extract with `tar -xvf k9s.tar.gz`
  - mv k9s to /usr/local/bin
 
- At this point I got scared of the additional complexities (for ip forwarding) of running minikube in a VM, I decided to run minikube locally first.
  - gave 4 CPUs and 10GB RAM
  