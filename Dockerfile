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

# Source
RUN git clone https://github.com/tectonic-typesetting/tectonic
WORKDIR /tectonic
RUN git submodule update --init

# Rust
RUN rustup default nightly
RUN rustup update

# Compile
WORKDIR ./fuzz
RUN sed -i '$ d' ./run-fuzzer.sh
RUN echo "cargo fuzz build" >> ./run-fuzzer.sh
RUN ./run-fuzzer.sh

# Run
CMD ["cargo", "fuzz", "run", "compile", "./corpus", "./seeds"]
