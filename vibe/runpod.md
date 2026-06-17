
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
    \"RUNPOD_USER_API_KEY\": \"$RUNPOD_API_KEY\"
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
