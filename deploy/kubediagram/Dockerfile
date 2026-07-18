FROM philippemerle/kubediagrams
ARG KUBECTL_VERSION=v1.33.4
RUN wget -q -O /usr/local/bin/kubectl "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/amd64/kubectl" \
    && chmod +x /usr/local/bin/kubectl
COPY render.sh /render.sh
COPY index.html /index.html
ENTRYPOINT ["/bin/sh", "/render.sh"]
