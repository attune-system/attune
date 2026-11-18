try {
  await Deno.readTextFile("/etc/passwd");
  throw new Error("Filesystem permission was unexpectedly granted");
} catch (error) {
  if (!(error instanceof Deno.errors.NotCapable)) throw error;
  console.log("permission-denied");
}
