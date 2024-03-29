////////////////////////////////////////////////////
////////////////// AUTO-GENERATED //////////////////
////////////////////////////////////////////////////

#![no_std]

dharitri_wasm_node::wasm_endpoints! {
    multisig
    (
        init
        deposit
        discardAction
        getActionData
        getActionLastIndex
        getActionSignerCount
        getActionSigners
        getActionValidSignerCount
        getAllBoardMembers
        getAllProposers
        getNumBoardMembers
        getNumProposers
        getPendingActionFullInfo
        getQuorum
        performAction
        proposeAddBoardMember
        proposeAddProposer
        proposeAsyncCall
        proposeChangeQuorum
        proposeRemoveUser
        proposeSCDeployFromSource
        proposeSCUpgradeFromSource
        proposeTransferExecute
        quorumReached
        sign
        signed
        unsign
        userRole
    )
}

dharitri_wasm_node::wasm_empty_callback! {}
