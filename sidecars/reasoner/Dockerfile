# syntax=docker/dockerfile:1
#
# OWL 2 DL reasoner sidecar (OWL API + HermiT), protocol v1.
# Build from the repository root (it copies LICENSES/ and NOTICE):
#
#   docker build -f sidecars/reasoner/Dockerfile -t open-triplestore-reasoner .
#
# docker-compose.yml runs it under the `reasoner` profile. HermiT, the OWL API
# and the other jars are shipped unmodified as separate files in /app/lib/
# (LGPL-3.0 §4: replace a jar to run a modified version); their licences are
# listed in /app/THIRD-PARTY.txt and the texts are in /app/LICENSES/.

FROM eclipse-temurin:21-jdk AS build
WORKDIR /src
COPY sidecars/reasoner/mvnw sidecars/reasoner/pom.xml ./
COPY sidecars/reasoner/.mvn .mvn
COPY sidecars/reasoner/src src
RUN --mount=type=cache,target=/root/.m2 ./mvnw -B -q package
# A runtime holding only the JDK modules the jars use.
RUN jdeps --ignore-missing-deps -q --multi-release 21 --print-module-deps \
        --class-path 'target/lib/*' target/ots-reasoner.jar target/lib/*.jar > modules \
 && jlink --add-modules "$(cat modules),jdk.crypto.ec,jdk.zipfs" \
        --strip-debug --no-man-pages --no-header-files --compress=zip-6 --output /jre \
 && mkdir /app && cp -r target/ots-reasoner.jar target/lib target/generated-sources/license/THIRD-PARTY.txt /app/

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin reasoner
COPY --from=build /jre /opt/java
COPY --from=build /app /app
COPY LICENSES /app/LICENSES
COPY NOTICE LICENSE /app/
ENV OTS_REASONER_PORT=8090 \
    JAVA_OPTS="-XX:MaxRAMPercentage=75 -XX:+ExitOnOutOfMemoryError"
USER reasoner
EXPOSE 8090
HEALTHCHECK --interval=30s --timeout=10s --start-period=20s --retries=3 \
    CMD ["/opt/java/bin/java", "-Xmx32m", "-jar", "/app/ots-reasoner.jar", "healthcheck"]
ENTRYPOINT ["sh", "-c", "exec /opt/java/bin/java $JAVA_OPTS -jar /app/ots-reasoner.jar \"$@\"", "--"]
