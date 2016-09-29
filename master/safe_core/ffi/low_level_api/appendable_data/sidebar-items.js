initSidebarItems({"enum":[["AppendableData","Wrapper for PrivAppendableData and PubAppendableData."],["FilterType","Filter Type"]],"fn":[["appendable_data_append","Append data."],["appendable_data_clear_data","Clear all data - moves it to deleted data."],["appendable_data_clear_deleted_data","Clear all deleted data - data will be actually be removed."],["appendable_data_encrypt_key","Get the owner's encrypt key"],["appendable_data_extract_data_id","Extract DataIdentifier from AppendableData."],["appendable_data_filter_type","Get the filter type"],["appendable_data_free","Free AppendableData handle"],["appendable_data_get","Get existing appendable data from Network."],["appendable_data_insert_to_filter","Insert a new entry to the (whitelist or blacklist) filter. If the key was already present in the filter, this is a no-op."],["appendable_data_is_owned","Returns true if the app is one of the owners of the provided AppendableData."],["appendable_data_new_priv","Create new PrivAppendableData"],["appendable_data_new_pub","Create new PubAppendableData"],["appendable_data_nth_data_id","Get nth appended DataIdentifier from data."],["appendable_data_nth_data_sign_key","Get nth sign key from data"],["appendable_data_nth_deleted_data_id","Get nth appended DataIdentifier from deleted data."],["appendable_data_nth_deleted_data_sign_key","Get nth sign key from deleted data"],["appendable_data_num_of_data","Get number of appended data items."],["appendable_data_num_of_deleted_data","Get number of appended deleted data items."],["appendable_data_post","POST appendable data (bumps the version)."],["appendable_data_put","PUT appendable data."],["appendable_data_remove_from_filter","Remove the given key from the (whitelist or blacklist) filter. If the key isn't present in the filter, this is a no-op."],["appendable_data_remove_nth_data","Remove the n-th data item from the appendable data. The data has to be POST'd afterwards for the change to be registered by the network. The data is moved to deleted data."],["appendable_data_remove_nth_deleted_data","Remove the n-th data item from the deleted data. The data has to be POST'd afterwards for the change to be registered by the network. The data is removed permanently."],["appendable_data_restore_nth_deleted_data","Restore the n-th delete data item to data field back. The data has to be POST'd afterwards for the change to be registered by the network."],["appendable_data_toggle_filter","Switch the filter of the appendable data."],["appendable_data_version","Get the current version of AppendableData by its handle"]]});