FROM fuzzers/cargo-fuzz:0.10.0

# Dependencies
RUN apt-get update \
 && apt-get install -yq \
	libfontconfig1-dev \
	libgraphite2-dev \
	libharfbuzz-dev \
	libicu-dev \
	libssl-dev \
	zlib1g-dev \
 && apt-get clean \
 && rm -rf /var/lib/apt/lists/*

# Rust
RUN rustup default nightly
RUN rustup update

# Source
ADD . /tectonic
RUN cd tectonic && git submodule update --init

# Compile
WORKDIR /tectonic/fuzz
RUN sed -i '$ d' ./run-fuzzer.sh
RUN echo "cargo fuzz build" >> ./run-fuzzer.sh
RUN ./run-fuzzer.sh

# Run
# CMD ["cargo", "fuzz", "run", "compile", "./corpus", "./seeds"]
CMD ["/tectonic/fuzz/target/x86_64-unknown-linux-gnu/release/compile"]
