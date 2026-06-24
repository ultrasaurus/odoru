
# Create a new RunPod template

Set `VERSION` to the new image tag before running. After the template is
created, update `TEMPLATE` in `vibe/.env` to the returned template `id`.

```bash
VERSION=v6

source vibe/.env
curl --request POST \
  --url https://rest.runpod.io/v1/templates \
  --header "Authorization: Bearer $RUNPOD_API_KEY" \
  --header 'Content-Type: application/json' \
  --data "{
  \"category\": \"CPU\",
  \"containerDiskInGb\": 20,
  \"dockerEntrypoint\": [],
  \"dockerStartCmd\": [],
  \"env\": {
    \"RUNPOD_USER_API_KEY\": \"$RUNPOD_API_KEY\",
    \"VIBE_SERVICE_SECRET\": \"$VIBE_SERVICE_SECRET\",
    \"GCS_BUCKET\": \"$GCS_BUCKET\",
    \"GCS_SA_KEY_B64\": \"$GCS_SA_KEY_B64\"
  },
  \"imageName\": \"dockersaura/vibe:$VERSION\",
  \"isPublic\": false,
  \"name\": \"vibe $VERSION\",
  \"ports\": [\"3000/http\",\"22/tcp\"],
  \"readme\": \"\",
  \"volumeInGb\": 0,
  \"volumeMountPath\": \"/workspace\"
}"
```

The response JSON includes `"id"` — copy that value into `TEMPLATE` in
`vibe/.env` so `new-pod` picks it up automatically.

## modify a template for new container version

```bash
VERSION=v8
source vibe/.env
curl --request PATCH \
  --url https://rest.runpod.io/v1/templates/$TEMPLATE \
  --header "Authorization: Bearer $RUNPOD_API_KEY" \
  --header 'Content-Type: application/json' \
  --data "{
  \"imageName\": \"dockersaura/vibe:$VERSION\",
  \"name\": \"vibe $VERSION\",
  \"env\": {
    \"RUNPOD_USER_API_KEY\": \"$RUNPOD_API_KEY\",
    \"VIBE_SERVICE_SECRET\": \"$VIBE_SERVICE_SECRET\",
    \"GCS_BUCKET\": \"$GCS_BUCKET\",
    \"GCS_SA_KEY_B64\": \"$GCS_SA_KEY_B64\"
  }
}"
```

Note: `source vibe/.env` first so `$RUNPOD_API_KEY`, `$TEMPLATE`,
`$VIBE_SERVICE_SECRET`, `$GCS_BUCKET`, and `$GCS_SA_KEY_B64` are all in
scope. `GCS_BUCKET` + `GCS_SA_KEY_B64` enable durable job state on RunPod —
`entrypoint.sh` decodes the key and sets `GCS_SA_KEY_PATH`; see
`dev/gcs-job-state.md`.


## delete template

```
export TEMPLATE_ID=put-your-template-id-here
curl --request DELETE \
  --url https://rest.runpod.io/v1/templates/$TEMPLATE_ID \
  --header "Authorization: Bearer $RUNPOD_API_KEY"
```

## list templates (find name/id pairs)

```
curl --request GET \
  --url https://rest.runpod.io/v1/templates \
  --header "Authorization: Bearer $RUNPOD_API_KEY"
```
