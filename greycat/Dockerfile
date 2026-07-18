FROM debian:bookworm-slim 
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates unzip \
    && curl -fsSL https://get.greycat.io/install.sh | bash -s stable \
    && ln -s /root/.greycat/bin/greycat /usr/local/bin/greycat \
    && greycat --version \
    && apt-get purge -y curl unzip && apt-get autoremove -y && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY project.gcl ./
COPY server ./server
RUN greycat install
EXPOSE 8080
# TODO : use authentication 
CMD ["greycat", "serve", "--user=1"]
