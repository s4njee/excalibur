FROM alpine:3.20
RUN apk add --no-cache nfs-utils
# Fixed ports so the container can be published predictably; the defaults are
# assigned dynamically via rpcbind, which does not survive port mapping.
RUN mkdir -p /exports/rw /exports/ro /exports/squash /exports/secure /run/rpc_pipefs
COPY exports /etc/exports
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
EXPOSE 111/tcp 2049/tcp 20048/tcp
ENTRYPOINT ["/entrypoint.sh"]
