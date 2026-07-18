#!/bin/sh
NS="${NAMESPACE:-wattopus}"
KINDS="deployments,replicasets,services,pods,configmaps,serviceaccounts"
mkdir -p /www
cp /index.html /www/index.html
( cd /www && exec python3 -m http.server "${PORT:-8000}" ) &

prev=""
while true; do
  cur=$(kubectl get deploy,rs,svc,pods -n "$NS" --no-headers \
        -o custom-columns='K:.kind,N:.metadata.name,R:.spec.replicas,P:.status.phase,NODE:.spec.nodeName' \
        2>/dev/null | md5sum)
  if [ -n "$cur" ] && [ "$cur" != "$prev" ]; then
    if kubectl get "$KINDS" -n "$NS" -o yaml > /tmp/state.yaml 2>/dev/null \
       && kube-diagrams -o /www/next.png /tmp/state.yaml >/dev/null 2>&1; then
      mv /www/next.png /www/diagram.png
      date -u +"%Y-%m-%dT%H:%M:%SZ" > /www/updated.txt
      # base64 wrapper: grafana's infinity ds pulls this server-side for the media panel
      { printf '{"updated":"%s","png":"' "$(cat /www/updated.txt)"
        base64 /www/diagram.png | tr -d '\n'
        printf '"}'
      } > /www/next.json
      mv /www/next.json /www/diagram.json
      prev="$cur"
      echo "rendered $(cat /www/updated.txt)"
    fi
  fi
  sleep "${INTERVAL:-10}"
done