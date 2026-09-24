# Test-only S3 provider. Keep the source release aligned with the previous
# registry fixture; Go's module checksum verification applies to this download.
FROM golang:1.24.13-alpine AS builder
ENV CGO_ENABLED=0 GOTOOLCHAIN=local GOMAXPROCS=2
RUN go install github.com/minio/minio@RELEASE.2025-09-07T16-13-09Z

FROM alpine:3.22.2
COPY --from=builder /go/bin/minio /usr/local/bin/minio
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
ENTRYPOINT ["/usr/local/bin/minio"]
