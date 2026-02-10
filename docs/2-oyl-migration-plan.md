# Oylapi Migration Plan

### Getting Started

Before reading this document, please familizarize yourself with the [OYL Overview](./1-overview.md) document. This document builds upon the concepts and context provided there.

#### TO CONTRIBUTE, use the oylapi-migration branch on espo

### Introduction

The goal of this plan is to outline the steps needed to implement the oylapi module, an informational module that mimics the functionality of the now defunct oyl api - with the plan of reverse proxying requests to Espo's oyl api implementation once finished.

Before implementing the `oylapi` module, there are a few weaknesses that must be addressed in espo to ensure the smooth implementation of the `oylapi` module and ensure espo is ready for production use as the core driver behind oyl api and oyl services.

Espo, as it is right now, has 3 main weaknesses:

1. **Lack of strict mode compliance**. Divergences exist on Espo from upstream Metashrew (espo must first complete a full index in **strict** mode to ensure all divergence points are fixed. It currently is not able to do so). We must **all collaborate** to find what assumptions Espo makes (specifically in **essentials/lib/balances**) that diverge from the functionality of that of Metashrew. Because espo doesnt use the balance indicies from metashrew and calcualtes it itself via its own implementation of the Alkane trasnfer rules (edicts, pointers, refund_pointers etc) - we must ensure that espo is able to run a complete index in strict mode with 0 divergence on **mainnet** before proceeding.

2. **Espo is reorg aware via bitcoind instead of metashrew**. Instead espo should be reorg aware via metashrew probing (metashrew keeps a non-append-only index of height -> block hash, which we can use directly to detect reorgs).

3. **Getters (db read operations) are decentralized across the database rather than centralized through an abstraction layer**. This is probably the simplest to fix but is a large refactor. Currently rpc methods on modules and the explorer itself read directly from **MDB (rocksdb handle)** rather than through defined getter methods. This will make the integration of the oylapi module difficult as there is no current centralized location of all of the operations supported by espo's index. Every module must be refactored to implement its own "getter" file, which exports all of the read operations for that module and takes in structured parameters rather than raw mdb handles and gives back structured, decoded data. This will make piecing together the **oylapi** module much easier.

Below is the migration plan I propose with every step we need to take to get espo from its current state to having the oylapi module fully implemented and ready for production use. Ill also include proposed asignees for each step, and if no asignees are listed yet you can volunteer to take that step head on.

### STEP 1 - Setup CI/CD for espo's rpc and explorer

**Proposed Asignee:** @flex

The first thing we should take care of is create a CI/CD attached to espos github repository @ https://github.com/bitapeslabs/espo public. This will make it so everyone is able to contribute and test the result of their changes without having a local electrs and metashrew db.

**Even better idea for flex**, setup a dedicated server with metashrew and electrs running - and then give contributors their own UNIX accounts to ssh in and test their changes directly on the server - where all users have access to the mount location of the metashrew and electrs dbs aswell as the ports the rpcs where these are running on. This will make debugging for everyone a lot easier.

### STEP 2 - Properly implement "Strict mode"

**Proposed Asignees:** @mork1e

Currently espo does have a strict mode (and a debug mode). These should be converged into one mode called "strict mode". Debug mode currently allows us to specify specific ontracts which at the end of every block on index_block is checked against metashrew to check for divergences.

Example: Currently on the consts file for essentials i am checking against the DIESEL/BUSD contract as it has the most activity. If there is a detected outflow from utils/balances/lib.rs that changes that contracts alkane balance in espo's indicies - we load the balance of an Alkane at that specific block from metashrew and compare it to the balance we have calculated in espo by the end of the block. If there is a divergence and the indexer is in debug mode, this will cause a panic.

the new "strict" mode should broaden this functionality and check all contracts for divergences, rather than just those on consts. It should also check for divergences against utxo balances. We load all utxos whose balance got written in espo's index from metashrew and then check for divergences between the utxos. If there is a mismatch, we panic.

Strict mode on panic should include detailed logging like what TX panicked, or which UTXO caused the panic, the actual divergences and balance sheets from both espo and metashrew, etc.

Once strict mode is implemented, we can properly start running espo in strict mode and catch all the insidious differences that currently exist between espo and metashrew

### STEP 3 - Refactor espo to be reorg aware via metashrew

**Proposed Asignees:** @flex

Currently espo is reorg aware via bitcoind. This should be changed to be reorg aware via metashrew probing via metashrew's height -> block hash index. This will completley eliminate all undefined behavior related to divergences between espo and metashrew.

**Strict mode** should be aware of this aswell (we only want to run strict mode during indexing because of this. rather than when when we are already at tip)

### STEP 4 - Refactor espo modules and explorer to use centralized getter methods

**Proposed Asignees:** Anyone!

Every module in espo should be refactored to implement its own "getter" file which exports all of the read operations for that module. Every rpc method in the module and explorer should be refactored to use these getter methods rather than reading directly from the mdb handle.

### STEP 5 - Extract one giant self describing .TS file that describes all of the methods oyl api currently implements.

**Proposed Asignees:** anyone!

This file should describe the types of all of the endpoints, the parameters they take in, and the data structures they return.

Evvery endpoint should have two base types, the "RequestParams" object and the "Response" object.

Example:

OylApiGetCandlesRequestParams {
query_params: {},
body_params: {
contract: string;
interval: string;
startTime?: number;
endTime?: number;
limit?: number;
}
}

OylApiGetCandlesResponse {
candles: Array<{
openTime: number;
open: string;
high: string;
low: string;
close: string;
volume: string;
closeTime: number;
}>;
}

this ts file should also have commented above it the actual endpoint path as it exists in oyl api currently. If some of these are query params, they should all be in the "query_params" field of the RequestParams object for that endpoint. If there are defaults, describe them in comments and make them optional in RequestParams.query_params.

The body_params field of RequestParams should contain all of the parameters that are sent in the body of a POST request to that endpoint (like query_params, if there are defaults describe them in comments and make them optional in body_params and comment the default).

### STEP 6- Create a markdown for an implementation plan for the oylapi module

**Proposed Asignees:** Anyone!

Using the newly created .TS file as context, have an LLM review the ts file and the current espo codebase to create a detailed implementation plan for every endpoint described in the ts file.

This implementation plan should describe for each endpoint:

- If espo's current indicies support the data required to fullfil that endpoint

- If they do, which espo modules will be involved in the implementation of that endpoint (and what getter methods will be used from those modules)

- If they do but the getter methods arent implemented yet, what new getter methods would need to be added to the oylapi module's getter file to support that endpoint

- If the indicies dont support that endpoint ina clean fashion (they simply dont support it or would require an O(n) scan of any of espos indicies), what new indicies would need to be created to support that endpoint in a reliable manner, and what would the implementation path be in the modules where these new indicies would live.

### STEP 7 - Implement the missing indicies and getters descrived in the markdown from step6

**Proposed Asignees:** Anyone!

Review the markdown file created in step6 to ensure it logically makes sense (have @mork1e review it as he has the most context of this database)

After changes are made and its iterated and approved, have an LLM use this markdown file to review all the getters and indicies it needs to implement marked in the markdown fie for implementation and have it start creation of the oylapi module (without the actual endpoint implementations yet just the module structure and getters).

The result of this step should be a fully working oylapi module (with no endpoints yet) that implements all of the missing indicies and getters described in the markdown from step 6.

Finally, rerun the indexer in strict mode to ensure 0 divergences exist after all of these changes.

### STEP 8 - Implement the endpoints in the oylapi module

**Proposed Asignees:** Anyone!

Using the markdown from step 6 as a guide, implement all of the endpoints in the oylapi module using the getters and indicies already implemented in step 7.

The oyl api is a REST api not an rpc, so you will need to implement:

-per module configuration in config.rs (so that every module can have its own configurations that are only enforced if that module is loaded)

- Make the oylapi's config require a host and port to run the REST api on.

- Serve the oylapi module as a REST api on the specified host and port when the module is loaded.

### STEP 9 - Finalization

**Proposed Asignees:** @flex, @drew

Once all endpoints are implemented as espo is fully reindexed and passes strict mode, reverse prooxy to the host and port the oylapi module is running on (and its REST api) and test to see if oyl wallet and the oyl app are back online and fully functional.
