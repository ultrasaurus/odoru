
```
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
  \"imageName\": \"dockersaura/vibe:v4\",
  \"isPublic\": false,
  \"name\": \"vibe v4\",
  \"ports\": [\"3000/http\",\"22/tcp\"],
  \"readme\": \"\",
  \"volumeInGb\": 0,
  \"volumeMountPath\": \"/workspace\"
}"
```