FROM ghcr.io/zwimer/tectonic:base

# Source
WORKDIR /
RUN rm -rf /tectonic
RUN git clone https://github.com/tectonic-typesetting/tectonic
RUN cd tectonic && git submodule update --init

# Compile
WORKDIR /tectonic/fuzz
RUN sed -i '$ d' ./run-fuzzer.sh
RUN echo "cargo fuzz build" >> ./run-fuzzer.sh
RUN ./run-fuzzer.sh

# Run
CMD ["cargo", "fuzz", "run", "compile", "./corpus", "./seeds"]
