SET search_path TO attune, public;

UPDATE pack_registry_index
SET url = 'https://raw.githubusercontent.com/attune-system/index/4c87ca62a4313f7e9646a50c44ab6b2b530e5f43/index.json',
    updated = NOW()
WHERE is_standard
  AND regexp_replace(
      url,
      '^https://raw[.]githubusercontent[.]com[.]?(:443)?/',
      'https://raw.githubusercontent.com/',
      'i'
  ) IN (
      'https://raw.githubusercontent.com/attune-system/index/main/index.json',
      'https://raw.githubusercontent.com/attune-system/index/793aabcc0eb537af7681a386b591de6c4fafd7a1/index.json',
      'https://raw.githubusercontent.com/attune-system/index/c9e48439677847797d056efb94ba1c855e188df9/index.json'
  );
